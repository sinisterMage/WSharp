// A return too wide for the machine's return registers.
//
// A value is a list of slots -- `!T` is a tag then `T`, and `?T` is a tag then
// `T` -- so `!?Row` is three, and x86-64 hands back two. Cranelift refuses such
// a signature rather than spilling one itself, so a return over the limit is
// written through a pointer the caller passes instead, immediately after the
// environment. That is the same bargain the runtime boundary already strikes
// for a builtin answering a `#[repr(C)]` pair, for a different reason: there it
// is what C does, here it is what the registers have room for.
//
// All three ways of calling a W# function have to agree about it, which is what
// this case is for: a static call, a dispatched one -- where every case writes
// one shared area and the join block carries nothing -- and an indirect one
// through a closure.
//
// The area is not a root and does not need to be. The callee writes it in the
// instructions before its `return` and the caller reads it in the instructions
// after the call, with no safepoint in between, so nothing it holds can go
// stale. `spin` below allocates hard either side of that window, so the
// `--gc-stress` pass collects inside every one of these calls.
// expect: err
// expect: none
// expect: 7
// expect: err
// expect: none
// expect: 14
// expect: err
// expect: none
// expect: 42
// expect: 5
// expect: 5
const array = @import("std/array");

const Row = struct { n: i64 };

const Base = struct { };
const Sub = struct : Base { n: i64 };

/// Allocate, so that a collection can happen anywhere one of these is called.
fn spin(n: i64) i64 {
    var a: []i64 = array.new(4);
    a[0] = n;
    return a[0];
}

// A static call.
fn plain(n: i64) !?Row {
    const m = spin(n);
    if (m < 0) { return error.Nope; }
    if (m == 0) { return null; }
    return Row{ .n = m };
}

// A dispatched call: two cases, one shared return area.
fn pick(b: Base) !?Row { return error.Nope; }
fn pick(s: Sub) !?Row {
    const m = spin(s.n);
    if (m == 0) { return null; }
    return Row{ .n = m * 2 };
}

fn show(v: !?Row) void {
    const r = v catch { print("err"); return; };
    if (r) |row| { print_int(row.n); } else { print("none"); }
}

fn main() i64 {
    show(plain(-1));
    show(plain(0));
    show(plain(7));

    const b: Base = Base{ };
    const s0: Base = Sub{ .n = 0 };
    const s7: Base = Sub{ .n = 7 };
    show(pick(b));
    show(pick(s0));
    show(pick(s7));

    // An indirect call, through a closure.
    const f = fn (n: i64) !?Row {
        const m = spin(n);
        if (m < 0) { return error.Nope; }
        if (m == 0) { return null; }
        return Row{ .n = m * 6 };
    };
    show(f(-1));
    show(f(0));
    show(f(7));

    // The payload really is the object that was made, not a word that happened
    // to survive: read a field out of it, twice, either side of an allocation.
    const kept = plain(5) catch return 1;
    if (kept) |row| {
        const n = row.n;
        const _spin = spin(1);
        print_int(row.n * 0 + n);
        print_int(row.n);
    }
    return 0;
}
