// Array operations.
//
// Written in W# rather than Rust because every one of these moves references
// from one object into another, and doing that correctly means going through
// the write barrier, the load barrier and the stack maps. Generated code does
// all three by construction; a runtime function would have to reproduce them,
// and a missed one loses objects instead of failing a test.
//
// `len` and `new` come from the builtin table: the first reads the header, and
// the second is lowered inline because only the call site knows the element
// type.

/// `a` followed by `b`.
fn concat[T](a: []T, b: []T) []T {
    var out = new(len(a) + len(b));
    var i = 0;
    for (a) |v| {
        out[i] = v;
        i += 1;
    }
    for (b) |v| {
        out[i] = v;
        i += 1;
    }
    return out;
}

/// `a` with `v` on the end.
///
/// A new array each time: an array's length is in its header, so growing one
/// in place would mean a capacity the type does not have.
fn push[T](a: []T, v: T) []T {
    var out = new(len(a) + 1);
    var i = 0;
    for (a) |x| {
        out[i] = x;
        i += 1;
    }
    out[i] = v;
    return out;
}

/// `a[from..to]`, clamped to the array rather than panicking.
///
/// Clamped because a slice is usually computed from a search that may have
/// found nothing, and every caller would otherwise have to check first.
fn slice[T](a: []T, from: i64, to: i64) []T {
    var start = from;
    if (start < 0) { start = 0; }
    if (start > len(a)) { start = len(a); }
    var end = to;
    if (end < start) { end = start; }
    if (end > len(a)) { end = len(a); }

    var out = new(end - start);
    var i = 0;
    while (i < end - start) : (i += 1) {
        out[i] = a[start + i];
    }
    return out;
}

/// `n` copies of `v`.
fn repeat[T](n: i64, v: T) []T {
    var count = n;
    if (count < 0) { count = 0; }
    var out = new(count);
    var i = 0;
    while (i < count) : (i += 1) { out[i] = v; }
    return out;
}
