// `catch` and `orelse` take a block, not only an expression.
//
// Two things are wanted on the right of one and they are not the same thing. A
// block whose last expression is written without a `;` is the value to use
// instead, after doing whatever had to be done first. A block that never
// produces a value has to *leave* -- `return`, `break`, `continue` -- and then
// it fits wherever it is written, which is what makes `f() catch return false`
// check in a function returning `bool` and in one returning `str` alike.
// expect: 6
// expect: true
// expect: false
// expect: 10
// expect: failed
// expect: 0
// expect: 6
// expect: nothing
// expect: -1
// expect: gave up
// expect: x
// expect: 8
fn risky(n: i64) !i64 {
    if (n < 0) { return error.Negative; }
    return n * 2;
}

// A one-statement alternative needs no braces; the `;` on the end belongs to
// the `const`, not to the `return`.
fn save(n: i64) bool {
    const v = risky(n) catch return false;
    print_int(v);
    return true;
}

fn loud(n: i64) i64 { return risky(n) catch { print("failed"); 0 }; }

// The same alternative, in a function whose return type is nothing like the
// other one's.
fn describe(n: i64) str {
    const v = risky(n) catch return "gave up";
    if (v > 0) { return "positive"; }
    return "zero";
}

fn main() i64 {
    print_bool(save(3));
    print_bool(save(-1));
    print_int(loud(5));
    print_int(loud(-2));

    // `continue` from inside an expression, which is the shape a loop over
    // fallible work wants.
    var i = 0;
    var seen = 0;
    while (i < 5) : (i += 1) {
        const v = risky(i - 2) catch continue;
        seen += v;
    }
    print_int(seen);

    var absent: ?i64 = null;
    print_int(absent orelse { print("nothing"); -1 });

    print(describe(-1));

    // A block may declare its own names, and they do not escape it.
    const v = 8;
    print_int(risky(-1) catch { const v = 99; if (v > 0) { print("x"); } 8 });
    return 0;
}
