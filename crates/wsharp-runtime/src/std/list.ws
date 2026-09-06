// A growable array.
//
// An array's length lives in its header, so there is no capacity beside it to
// grow into -- `array.push` allocates a whole new array every call. A `List`
// is the second object the length needs: `items` is the backing array, whose
// header length is the *capacity*, and `count` is how many of its slots are
// in use. Pushing writes into the spare tail and is amortised constant time.
//
// Written in W# rather than Rust for the usual reason: every one of these
// moves references from one object into another, and generated code goes
// through the write barrier, the load barrier and the stack maps by
// construction. See `std/array.ws`.
const array = @import("std/array");

/// A sequence that grows.
///
/// `items` is over-allocated: only the first `count` slots hold values the
/// list is claiming. The rest read as null, because a hole is zeroed when an
/// allocator takes it, which is exactly what the collector expects to find.
pub const List = struct[T] { items: []T, count: i64 };

/// An empty list, holding no backing array at all.
///
/// The element type comes from what the list is used at, so
/// `var xs = list.new(); list.push(xs, 1);` needs no annotation: a local
/// binding is monomorphic, and `push` is what pins `T`.
pub fn new[T]() List[T] {
    return List{ .items = []T{}, .count = 0 };
}

/// An empty list with room for `n` before it has to grow.
pub fn with_capacity[T](n: i64) List[T] {
    var room = n;
    if (room < 0) { room = 0; }
    var items: []T = array.new(room);
    return List{ .items = items, .count = 0 };
}

/// A list holding a copy of `a`.
///
/// A copy rather than a borrow: the list would otherwise write through a
/// caller's array, and `a` has no capacity of its own to grow into anyway.
pub fn from[T](a: []T) List[T] {
    var out: []T = array.new(array.len(a));
    var i = 0;
    for (a) |v| {
        out[i] = v;
        i += 1;
    }
    return List{ .items = out, .count = array.len(a) };
}

/// How many values `l` holds.
pub fn len[T](l: List[T]) i64 {
    return l.count;
}

/// How many `l` can hold before it next has to grow.
pub fn capacity[T](l: List[T]) i64 {
    return array.len(l.items);
}

/// `l[i]`.
///
/// Bounds-checked against the count rather than the capacity: indexing the
/// backing array directly would happily hand back a spare slot. The message
/// is the one an array gives, because there is no reason for a list to say
/// less about the same mistake.
pub fn get[T](l: List[T], i: i64) T {
    if (i < 0 or i >= l.count) { panic_index(i, l.count); }
    return l.items[i];
}

/// `l[i] = v`.
pub fn set[T](l: List[T], i: i64, v: T) void {
    if (i < 0 or i >= l.count) { panic_index(i, l.count); }
    l.items[i] = v;
}

/// Append `v`, growing the backing array if it is full.
///
/// Doubling is what makes a run of pushes amortised constant time: each grow
/// copies everything, but the copies are paid for by the pushes that fit.
pub fn push[T](l: List[T], v: T) void {
    reserve(l, l.count + 1);
    l.items[l.count] = v;
    l.count = l.count + 1;
}

/// Append every element of `a`.
pub fn extend[T](l: List[T], a: []T) void {
    reserve(l, l.count + array.len(a));
    for (a) |v| {
        l.items[l.count] = v;
        l.count = l.count + 1;
    }
}

/// Remove and return the last value.
///
/// Panics on an empty list, for the same reason `a[i]` panics rather than
/// returning `?T`: an `orelse` in every caller costs more than the failures
/// it catches.
pub fn pop[T](l: List[T]) T {
    if (l.count == 0) { panic_index(0, 0); }
    l.count = l.count - 1;
    return l.items[l.count];
}

/// Insert `v` at `i`, shifting everything from there along.
///
/// `i` may be the count, which appends; anything past it is out of range.
pub fn insert[T](l: List[T], i: i64, v: T) void {
    if (i < 0 or i > l.count) { panic_index(i, l.count); }
    reserve(l, l.count + 1);
    var at = l.count;
    while (at > i) : (at -= 1) {
        l.items[at] = l.items[at - 1];
    }
    l.items[i] = v;
    l.count = l.count + 1;
}

/// Remove and return the value at `i`, shifting everything after it back.
pub fn remove[T](l: List[T], i: i64) T {
    if (i < 0 or i >= l.count) { panic_index(i, l.count); }
    const gone = l.items[i];
    var at = i;
    while (at < l.count - 1) : (at += 1) {
        l.items[at] = l.items[at + 1];
    }
    l.count = l.count - 1;
    return gone;
}

/// Drop everything `l` holds.
///
/// The backing array goes too, rather than only the count: a list that has
/// been cleared is usually one that is finished with, and keeping the array
/// would keep every value in it alive.
pub fn clear[T](l: List[T]) void {
    l.items = []T{};
    l.count = 0;
}

/// A walk over `l`, from the front.
///
/// `at` is the next index to hand out. Holding the list rather than its
/// backing array is what makes `next` see a `push` that happened mid-loop --
/// and see the count that goes with it, which a captured array could not.
pub const Iter = struct[T] { list: List[T], at: i64 };

/// Where a `for (l) |v|` starts.
///
/// `for` over anything but an array calls `iter` and then `next` until it
/// produces null, and resolves both in the module that declares the type it
/// is walking. So this is the whole of what makes a list iterable, and
/// `to_array` is no longer the way to write the loop.
pub fn iter[T](l: List[T]) Iter[T] {
    return Iter{ .list = l, .at = 0 };
}

/// The next value, or null at the end.
pub fn next[T](it: Iter[T]) ?T {
    if (it.at >= it.list.count) { return null; }
    const v = it.list.items[it.at];
    it.at = it.at + 1;
    return v;
}

/// The values `l` holds, as an array of exactly `count` elements.
///
/// A copy, because the backing array is longer than the list and handing it
/// out would expose the spare slots.
pub fn to_array[T](l: List[T]) []T {
    var out: []T = array.new(l.count);
    var i = 0;
    while (i < l.count) : (i += 1) {
        out[i] = l.items[i];
    }
    return out;
}

/// Make sure `l` can hold `want` values without growing again.
///
/// Private: growing is this module's business, and a caller that reserved the
/// wrong amount would be a caller that had to know about the doubling.
///
/// Doubling from four: growing one slot at a time would make a run of pushes
/// quadratic, which is the whole reason this type exists.
fn reserve[T](l: List[T], want: i64) void {
    var room = array.len(l.items);
    if (want <= room) { return; }

    if (room == 0) { room = 4; }
    while (room < want) : (room = room * 2) { }

    var bigger: []T = array.new(room);
    var i = 0;
    while (i < l.count) : (i += 1) {
        bigger[i] = l.items[i];
    }
    l.items = bigger;
}
