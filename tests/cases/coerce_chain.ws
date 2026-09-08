// A coercion is a sequence of steps, not one step.
//
// Widening a subtype into its supertype and wrapping a value into a `?T` or an
// `!T` were each supported and were not composed, so `return Sub{ .. };` from a
// function declared `!Base` was a type error and the spelling was an annotated
// binding per conversion. Two wraps were not composed either, which is what
// made `!?T` unwritable.
//
// Nothing is emitted for the widening half -- a subtype's layout starts with a
// copy of its supertype's, and every struct is one pointer slot -- so the value
// that comes out the far end is still the subtype it went in as, which is what
// the dispatched calls below check.
// expect: sub
// expect: sub
// expect: sub
// expect: 0
// expect: 1
// expect: none
// expect: err
const Base = struct { };
const Sub = struct : Base { n: i64 };

fn name(b: Base) str { return "base"; }
fn name(s: Sub) str { return "sub"; }

// Widen, then wrap.
fn in_error() !Base { return Sub{ .n = 1 }; }
fn in_option() ?Base { return Sub{ .n = 2 }; }

// Wrap twice, and widen on the way: `Sub` is not a `?Base` and a `?Base` is
// not a `!?Base`, and this is both steps and the widening at once.
fn in_both() !?Base { return Sub{ .n = 3 }; }

// The shape a cursor wants: an answer, an end, or a failure, in one call.
const Row = struct { n: i64 };
fn row_at(i: i64) !?Row {
    if (i > 2) { return error.Empty; }
    if (i == 2) { return null; }
    return Row{ .n = i };
}

fn main() i64 {
    print(name(in_error() catch return 1));
    print(name(in_option() orelse return 2));
    const both = in_both() catch return 3;
    print(name(both orelse return 4));

    var i = 0;
    while (i < 4) : (i += 1) {
        const r = row_at(i) catch { print("err"); continue; };
        if (r) |row| { print_int(row.n); } else { print("none"); }
    }
    return 0;
}
