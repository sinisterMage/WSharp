// A hash table with `str` keys.
//
// Written in W# rather than Rust for `std/list`'s reason, one level up: every
// operation here moves references from one object into another, and doing that
// correctly means going through the write barrier, the load barrier and the
// stack maps. Generated code does all three by construction.
//
// `std` went a long time without one, and said so out loud in two places --
// `std/toml` scans parallel key and value lists, `ingot/pubgrub` keeps a list
// "for `std`'s reason: there is no map". Association lists are right for a
// dozen headers and wrong for a prepared-statement cache or a session store,
// and the fix for those was always a map rather than a cleverer scan.
//
// Open addressing with linear probing, over three parallel arrays and a state
// byte, rather than buckets of chained entries: a chain is one allocation per
// entry and one pointer chase per probe, and the collector has to walk both.
// Linear probing keeps a lookup to one array read in the common case and its
// neighbours in the same cache line.
//
// Keys are `str` only. That is what `==` can compare and what `str.hash` can
// hash, and it is what the callers this exists for have -- a header name, a
// SQL statement, a session id, a package name.
const array = @import("std/array");
const text = @import("std/str");

/// What a slot holds.
///
/// A byte rather than a null key, because a `str` field cannot be written null
/// unless it is an optional and an empty string is a perfectly good key. The
/// three states are not two: a probe stops at `EMPTY` and must *not* stop at
/// `DEAD`, or a removal would hide every key that had probed past it.
const EMPTY: u8 = 0;
const LIVE: u8 = 1;
const DEAD: u8 = 2;

/// A mapping from `str` to `V`.
///
/// `count` is what `len` answers and `used` is `count` plus the tombstones,
/// which is what the load factor has to watch: a table that filled and emptied
/// repeatedly would otherwise probe through a run of `DEAD` slots for ever
/// without ever growing.
pub const Map = struct[V] {
    keys: []str,
    vals: []V,
    hashes: []u64,
    state: []u8,
    count: i64,
    used: i64,
};

/// An empty map, holding no arrays at all.
///
/// The value type comes from what the map is used at, so
/// `var m = map.new(); map.set(m, "a", 1);` needs no annotation -- a local
/// binding is monomorphic and `set` is what pins `V`.
pub fn new[V]() Map[V] {
    return Map{
        .keys = []str{}, .vals = []V{}, .hashes = []u64{}, .state = []u8{},
        .count = 0, .used = 0,
    };
}

/// An empty map with room for `n` entries before it has to grow.
///
/// Rounded up to a power of two and then to twice `n`, because the table grows
/// at seven eighths and a caller asking for room for `n` means room for `n`.
pub fn with_capacity[V](n: i64) Map[V] {
    var m: Map[V] = new();
    if (n > 0) { resize(m, table_size(n * 2)); }
    return m;
}

/// How many entries `m` holds.
pub fn len[V](m: Map[V]) i64 {
    return m.count;
}

/// The value `key` names, or null.
pub fn get[V](m: Map[V], key: str) ?V {
    const at = find(m, key);
    if (at < 0) { return null; }
    return m.vals[at];
}

/// Whether `m` holds `key`.
///
/// Beside `get` rather than written as `get(m, k) != null`, because `?V` of an
/// optional value type cannot be told apart from a missing entry that way.
pub fn has[V](m: Map[V], key: str) bool {
    return find(m, key) >= 0;
}

/// Bind `key` to `value`, replacing whatever it named.
pub fn set[V](m: Map[V], key: str, value: V) void {
    // Grown before the insert rather than after, so the slot chosen below is
    // one in the table this entry will actually live in.
    //
    // Seven eighths, and against `used` rather than `count`: linear probing
    // degrades sharply as a table fills, and a tombstone costs a probe exactly
    // as a live entry does.
    const room = array.len(m.state);
    if ((m.used + 1) * 8 >= room * 7) {
        // Twice the *live* count, so a table that is mostly tombstones is
        // cleaned rather than doubled.
        resize(m, table_size((m.count + 1) * 2));
    }

    const hash = text.hash(key);
    const mask = array.len(m.state) - 1;
    var i = i64(hash & u64(mask));
    // The first tombstone passed, so an insert reuses it rather than growing
    // the probe run -- but only after the whole run has been searched, or the
    // same key could be stored twice.
    var free = -1;
    while (true) {
        const s = m.state[i];
        if (s == EMPTY) {
            var at = i;
            if (free >= 0) {
                at = free;
            } else {
                // A fresh slot, rather than a tombstone reused: this is the
                // only path that makes the probe run longer.
                m.used += 1;
            }
            m.keys[at] = key;
            m.vals[at] = value;
            m.hashes[at] = hash;
            m.state[at] = LIVE;
            m.count += 1;
            return;
        }
        if (s == DEAD) {
            if (free < 0) { free = i; }
        } else {
            if (m.hashes[i] == hash and text.eq(m.keys[i], key)) {
                m.vals[i] = value;
                return;
            }
        }
        i = (i + 1) & mask;
    }
    return;
}

/// Forget `key`. Answers whether it was there.
///
/// The slot becomes a tombstone rather than empty, because emptying it would
/// end a probe run that other keys are still reached through.
///
/// The key and the value stay reachable until the slot is reused, exactly as
/// `list.pop` leaves a dead reference in the tail: `m.keys[i] = null` only
/// typechecks when the element type is an optional, and the collector walks
/// every element the array header claims. Known, and documented rather than
/// fixed by pretending otherwise.
pub fn remove[V](m: Map[V], key: str) bool {
    const at = find(m, key);
    if (at < 0) { return false; }
    m.state[at] = DEAD;
    m.count -= 1;
    return true;
}

/// Every key `m` holds, in the table's order -- which is the hash's, and is
/// neither insertion order nor sorted.
pub fn keys[V](m: Map[V]) []str {
    var out: []str = array.new(m.count);
    const room = array.len(m.state);
    var i = 0;
    var n = 0;
    while (i < room) : (i += 1) {
        if (m.state[i] == LIVE) {
            out[n] = m.keys[i];
            n += 1;
        }
    }
    return out;
}

/// Forget everything, keeping the capacity for the next use.
pub fn clear[V](m: Map[V]) void {
    const room = array.len(m.state);
    var i = 0;
    while (i < room) : (i += 1) { m.state[i] = EMPTY; }
    m.count = 0;
    m.used = 0;
    return;
}

/// One entry, as a walk hands it back.
pub const Entry = struct[V] { key: str, value: V };

/// A walk over `m`.
///
/// `at` is the next slot to look at. Adding to a map while walking it is not a
/// thing to do: an insert may grow the table, and then this index means
/// nothing.
pub const Iter = struct[V] { map: Map[V], at: i64 };

/// Where a `for (m) |e|` starts.
pub fn iter[V](m: Map[V]) Iter[V] {
    return Iter{ .map = m, .at = 0 };
}

/// The next entry, or null at the end.
pub fn next[V](it: Iter[V]) ?Entry[V] {
    const room = array.len(it.map.state);
    while (it.at < room) : (it.at = it.at + 1) {
        if (it.map.state[it.at] == LIVE) {
            const e = Entry{ .key = it.map.keys[it.at], .value = it.map.vals[it.at] };
            it.at = it.at + 1;
            return e;
        }
    }
    return null;
}

/// The slot `key` lives in, or -1.
///
/// Private: an index into the table is not a thing to hand out, because the
/// next `set` may move it.
fn find[V](m: Map[V], key: str) i64 {
    const room = array.len(m.state);
    if (room == 0) { return -1; }
    const hash = text.hash(key);
    const mask = room - 1;
    var i = i64(hash & u64(mask));
    // Bounded by the table, not by the probe run: a table that is entirely
    // `LIVE` and `DEAD` has no `EMPTY` to stop at, and `set` keeps that from
    // happening -- but a loop that trusts it would hang rather than fail if it
    // ever did.
    var steps = 0;
    while (steps < room) : (steps += 1) {
        const s = m.state[i];
        if (s == EMPTY) { return -1; }
        // The stored hash first: comparing two strings is a call, and two keys
        // in one probe run usually differ here.
        if (s == LIVE and m.hashes[i] == hash and text.eq(m.keys[i], key)) {
            return i;
        }
        i = (i + 1) & mask;
    }
    return -1;
}

/// The smallest power of two that is at least `want`, and at least 8.
///
/// A power of two so that the mask is `size - 1` and the wrap is an `and`
/// rather than a `%`, which is a division.
fn table_size(want: i64) i64 {
    var size = 8;
    while (size < want) : (size = size * 2) { }
    return size;
}

/// Move every live entry into a table of `size` slots.
///
/// Tombstones do not travel, which is what makes a resize the thing that cleans
/// them up -- and why `set` measures its load against `used` rather than
/// `count`, so a table that is mostly tombstones reaches this.
fn resize[V](m: Map[V], size: i64) void {
    const old_keys = m.keys;
    const old_vals = m.vals;
    const old_hashes = m.hashes;
    const old_state = m.state;
    const old_room = array.len(old_state);

    m.keys = array.new(size);
    m.vals = array.new(size);
    m.hashes = array.new(size);
    m.state = array.new(size);
    m.count = 0;
    m.used = 0;

    const mask = size - 1;
    var i = 0;
    while (i < old_room) : (i += 1) {
        if (old_state[i] == LIVE) {
            // Re-probed rather than re-hashed: the hash was stored for exactly
            // this, and hashing every key again would make a resize cost the
            // length of every key in the table.
            const hash = old_hashes[i];
            var j = i64(hash & u64(mask));
            while (m.state[j] == LIVE) : (j = (j + 1) & mask) { }
            m.keys[j] = old_keys[i];
            m.vals[j] = old_vals[i];
            m.hashes[j] = hash;
            m.state[j] = LIVE;
            m.count += 1;
            m.used += 1;
        }
    }
    return;
}
