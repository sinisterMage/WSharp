// A git packfile, read.
//
// The format is a header, a run of objects each compressed with zlib, and a
// SHA-1 trailer. Two of the six object types are *deltas* -- a patch against
// another object in the same pack, named either by how far back it is or by its
// id -- and resolving those is where the surprises are.
//
// Three things here are worth knowing before reading the code, because each is
// a place an obvious implementation is wrong.
//
//   * **A packfile is a concatenation of zlib streams with nothing between
//     them.** Only the decompressor knows where one ends, which is why
//     `std/inflate` answers with a cursor rather than with bytes.
//   * **An offset delta's varint is not the size varint.** Sizes are the
//     ordinary seven-bits-at-a-time little-endian form; an offset accumulates
//     `((n + 1) << 7) | next`, which makes every encoding of a given number
//     unique. Reading one with the other's loop gives a plausible wrong
//     answer, and is the classic bug.
//   * **A copy instruction with a size of zero means 65536.** Zero would be a
//     copy of nothing, which no encoder emits, so the value was reused.
const array = @import("std/array");
const bytes = @import("std/bytes");
const fault = @import("ingot/fault");
const hash = @import("std/hash");
const inflate = @import("std/inflate");
const list = @import("std/list");
const text = @import("std/str");

pub const COMMIT = 1;
pub const TREE = 2;
pub const BLOB = 3;
pub const TAG = 4;
const OFS_DELTA = 6;
const REF_DELTA = 7;

/// One object out of a pack, with its deltas already applied.
pub const Object = struct { kind: i64, data: []u8, id: str };

/// What an object of each type is called, which is also what its id is
/// computed over.
pub fn type_name(kind: i64) str {
    if (kind == COMMIT) { return "commit"; }
    if (kind == TREE) { return "tree"; }
    if (kind == BLOB) { return "blob"; }
    if (kind == TAG) { return "tag"; }
    return "";
}

/// A git object's id: SHA-1 over `"<type> <size>\0"` and the contents.
pub fn object_id(kind: i64, data: []u8) str {
    const s = hash.sha1_init();
    const header = bytes.of(text.concat(text.concat(type_name(kind), " "),
        text.concat(text.from_int(array.len(data)), "\0")));
    hash.sha1_update(s, header, 0, array.len(header));
    hash.sha1_update(s, data, 0, array.len(data));
    return bytes.to_hex(hash.sha1_final(s));
}

/// An entry as the first pass found it: decompressed, but perhaps still a
/// patch against something else.
const Entry = struct {
    offset: i64,
    kind: i64,
    data: []u8,
    /// Where the base is, for an offset delta; -1 otherwise.
    base_offset: i64,
    /// The base's id, for a reference delta; `""` otherwise.
    base_id: str,
    done: bool,
};

/// Every object a packfile holds.
pub fn read(f: fault.Fault, pack: []u8) ?list.List[Object] {
    const n = array.len(pack);
    if (n < 32) { fault.fail(f, "the packfile is too short to be one"); return null; }
    if (pack[0] != 80 or pack[1] != 65 or pack[2] != 67 or pack[3] != 75) {
        fault.fail(f, "the packfile does not begin with `PACK`");
        return null;
    }
    const version = i64(bytes.be32(pack, 4));
    if (version != 2 and version != 3) {
        fault.fail(f, text.concat("this packfile is version ", text.from_int(version)));
        return null;
    }
    const count = i64(bytes.be32(pack, 8));

    // The trailer is a SHA-1 of everything before it. Checked, because a
    // truncated download is the ordinary failure and every other symptom of it
    // is worse than this one.
    const body = n - 20;
    if (body < 12) { fault.fail(f, "the packfile is truncated"); return null; }
    const s = hash.sha1_init();
    hash.sha1_update(s, pack, 0, body);
    if (!bytes.equal(hash.sha1_final(s), bytes.slice(pack, body, n))) {
        fault.fail(f, "the packfile's checksum does not match its contents");
        return null;
    }

    var entries: list.List[Entry] = list.new();
    var at = 12;
    var i = 0;
    while (i < count) : (i += 1) {
        const entry = one(f, pack, at, body) orelse return null;
        at = entry.offset;
        list.push(entries, entry.value);
    }
    return resolve(f, entries);
}

/// An entry, and where reading it left off.
const Read = struct { value: Entry, offset: i64 };

fn one(f: fault.Fault, pack: []u8, start: i64, body: i64) ?Read {
    var at = start;
    if (at >= body) { fault.fail(f, "the packfile ends in the middle of an object"); return null; }
    const c = i64(pack[at]);
    at += 1;
    const kind = (c >> 4) & 7;
    var size = c & 15;
    var shift = 4;
    var more = c & 0x80;
    while (more != 0) {
        if (at >= body) { fault.fail(f, "the packfile ends in the middle of a size"); return null; }
        const b = i64(pack[at]);
        at += 1;
        size = size | ((b & 0x7f) << shift);
        shift += 7;
        more = b & 0x80;
    }

    var base_offset = -1;
    var base_id = "";
    if (kind == OFS_DELTA) {
        // The accumulating form, and not the one sizes use.
        if (at >= body) { fault.fail(f, "the packfile ends in the middle of a delta offset"); return null; }
        var b = i64(pack[at]);
        at += 1;
        var back = b & 0x7f;
        while (b & 0x80 != 0) {
            if (at >= body) { fault.fail(f, "the packfile ends in the middle of a delta offset"); return null; }
            b = i64(pack[at]);
            at += 1;
            back = ((back + 1) << 7) | (b & 0x7f);
        }
        base_offset = start - back;
        if (base_offset < 12) {
            fault.fail(f, "a delta points at something before the first object");
            return null;
        }
    } else {
        if (kind == REF_DELTA) {
            if (at + 20 > body) { fault.fail(f, "the packfile ends in the middle of a base id"); return null; }
            base_id = bytes.to_hex(bytes.slice(pack, at, at + 20));
            at += 20;
        } else {
            if (kind < COMMIT or kind > TAG) {
                fault.fail(f, text.concat("a packfile object has type ", text.from_int(kind)));
                return null;
            }
        }
    }

    const out = bytes.buf(size + 16);
    const after = inflate.zlib(pack, at, out) catch {
        fault.fail(f, "a packfile object is not a zlib stream");
        return null;
    };
    const data = bytes.taken(out);
    if (array.len(data) != size) {
        fault.fail(f, "a packfile object is not the size it says it is");
        return null;
    }
    return Read{
        .value = Entry{
            .offset = start,
            .kind = kind,
            .data = data,
            .base_offset = base_offset,
            .base_id = base_id,
            .done = kind != OFS_DELTA and kind != REF_DELTA,
        },
        .offset = after,
    };
}

/// Apply every delta until none is left.
///
/// Passes rather than recursion: an offset delta always points backwards, so
/// one pass would do for those alone, but a reference delta may name a base
/// that comes later. A pass that resolves nothing means what is left cannot be
/// resolved -- a thin pack, or a cycle -- and is refused rather than looped on.
fn resolve(f: fault.Fault, entries: list.List[Entry]) ?list.List[Object] {
    const n = list.len(entries);
    var left = 0;
    var i = 0;
    while (i < n) : (i += 1) {
        if (!list.get(entries, i).done) { left += 1; }
    }
    while (left > 0) {
        var progressed = false;
        i = 0;
        while (i < n) : (i += 1) {
            const e = list.get(entries, i);
            if (e.done) { continue; }
            const base = find_base(entries, e) orelse continue;
            if (!base.done) { continue; }
            const patched = apply(f, base.data, e.data) orelse return null;
            e.kind = base.kind;
            e.data = patched;
            e.done = true;
            left -= 1;
            progressed = true;
        }
        if (!progressed) {
            fault.fail(f, "a delta in this packfile has no base in it");
            return null;
        }
    }

    var out: list.List[Object] = list.new();
    i = 0;
    while (i < n) : (i += 1) {
        const e = list.get(entries, i);
        list.push(out, Object{ .kind = e.kind, .data = e.data, .id = object_id(e.kind, e.data) });
    }
    return out;
}

fn find_base(entries: list.List[Entry], e: Entry) ?Entry {
    const n = list.len(entries);
    var i = 0;
    while (i < n) : (i += 1) {
        const other = list.get(entries, i);
        if (e.base_offset >= 0) {
            if (other.offset == e.base_offset) { return other; }
        } else {
            if (other.done and text.eq(object_id(other.kind, other.data), e.base_id)) { return other; }
        }
    }
    return null;
}

/// One object patched into another.
pub fn apply(f: fault.Fault, base: []u8, delta: []u8) ?[]u8 {
    const n = array.len(delta);
    var at = 0;
    const want_base = varint(delta, at);
    at = want_base.at;
    const want_size = varint(delta, at);
    at = want_size.at;
    if (want_base.value != array.len(base)) {
        fault.fail(f, "a delta expects a base of a different size");
        return null;
    }

    const out = bytes.buf(want_size.value + 16);
    while (at < n) {
        const c = i64(delta[at]);
        at += 1;
        if (c & 0x80 != 0) {
            var offset = 0;
            var size = 0;
            if (c & 0x01 != 0) { offset = offset | at_byte(delta, at); at += 1; }
            if (c & 0x02 != 0) { offset = offset | (at_byte(delta, at) << 8); at += 1; }
            if (c & 0x04 != 0) { offset = offset | (at_byte(delta, at) << 16); at += 1; }
            if (c & 0x08 != 0) { offset = offset | (at_byte(delta, at) << 24); at += 1; }
            if (c & 0x10 != 0) { size = size | at_byte(delta, at); at += 1; }
            if (c & 0x20 != 0) { size = size | (at_byte(delta, at) << 8); at += 1; }
            if (c & 0x40 != 0) { size = size | (at_byte(delta, at) << 16); at += 1; }
            // Zero would be a copy of nothing, which no encoder writes, so the
            // value was given a use.
            if (size == 0) { size = 65536; }
            if (at > n or offset < 0 or offset + size > array.len(base)) {
                fault.fail(f, "a delta copies from outside its base");
                return null;
            }
            bytes.put_bytes(out, base, offset, size);
            continue;
        }
        if (c == 0) {
            fault.fail(f, "a delta holds an instruction that is reserved");
            return null;
        }
        if (at + c > n) {
            fault.fail(f, "a delta ends in the middle of an insertion");
            return null;
        }
        bytes.put_bytes(out, delta, at, c);
        at += c;
    }
    const result = bytes.taken(out);
    if (array.len(result) != want_size.value) {
        fault.fail(f, "a delta produced something of the wrong size");
        return null;
    }
    return result;
}

fn at_byte(b: []u8, at: i64) i64 {
    if (at >= array.len(b)) { return 0; }
    return i64(b[at]);
}

const Varint = struct { value: i64, at: i64 };

/// The ordinary seven-bits-at-a-time little-endian form that sizes use.
fn varint(b: []u8, from: i64) Varint {
    var at = from;
    var value = 0;
    var shift = 0;
    var more = 1;
    while (more != 0 and at < array.len(b)) {
        const c = i64(b[at]);
        at += 1;
        value = value | ((c & 0x7f) << shift);
        shift += 7;
        more = c & 0x80;
    }
    return Varint{ .value = value, .at = at };
}

// ---------------------------------------------------------------------------
// Trees
// ---------------------------------------------------------------------------

/// One entry of a tree object.
pub const TreeEntry = struct { mode: str, name: str, id: str, is_dir: bool };

/// A tree object, cut up: `<mode> <name>\0<20 raw bytes of id>`, repeated.
///
/// The id is *raw* here and hex everywhere else, which is the one place git's
/// two spellings of the same twenty bytes meet.
pub fn parse_tree(f: fault.Fault, data: []u8) ?list.List[TreeEntry] {
    var out: list.List[TreeEntry] = list.new();
    const n = array.len(data);
    var at = 0;
    while (at < n) {
        var space = at;
        while (space < n and data[space] != 32) : (space += 1) { }
        if (space >= n) { fault.fail(f, "a tree entry has no mode"); return null; }
        const mode = bytes.slice_str(data, at, space);
        var stop = space + 1;
        while (stop < n and data[stop] != 0) : (stop += 1) { }
        if (stop + 21 > n) { fault.fail(f, "a tree entry has no id"); return null; }
        const name = bytes.slice_str(data, space + 1, stop);
        const id = bytes.to_hex(bytes.slice(data, stop + 1, stop + 21));
        // `40000` is a directory; everything else here is a file of some kind.
        list.push(out, TreeEntry{
            .mode = mode,
            .name = name,
            .id = id,
            .is_dir = text.eq(mode, "40000"),
        });
        at = stop + 21;
    }
    return out;
}

/// The tree a commit names.
pub fn commit_tree(f: fault.Fault, data: []u8) ?str {
    const header = bytes.slice_str(data, 0, smaller(array.len(data), 200));
    if (!text.starts_with(header, "tree ")) {
        fault.fail(f, "a commit does not begin with its tree");
        return null;
    }
    const id = text.substr(header, 5, 45);
    if (text.len(id) != 40) { fault.fail(f, "a commit names a tree that is not an id"); return null; }
    return id;
}

fn smaller(a: i64, b: i64) i64 { if (a < b) { return a; } return b; }

/// The object with this id, or null.
pub fn find(objects: list.List[Object], id: str) ?Object {
    for (list.to_array(objects)) |o| {
        if (text.eq(o.id, id)) { return o; }
    }
    return null;
}
