// Handing a caught error back on.
//
// `catch |e|` bound the error and there was nowhere to put it: an error union
// carries a tag, and the only thing that could build one was an `error.X`
// written in the source. So a function that wanted to handle one case and pass
// the rest along could not, and the shape it had to take instead was to ask
// enough questions *before* the call to be sure it would not raise -- which is
// why `std/fs.mkdir_all` tests for a directory rather than catching
// `AlreadyExists`.
//
// An `error` value and an `!T` share their first slot and its encoding -- the
// error's index plus one, so that zero can mean success -- so this costs one
// move. The set still has to be a subset of what the function may raise, which
// is the same edge `try` contributes and gives the same diagnostic.
// expect: 0
// expect: handled One
// expect: 111
// expect: outer caught Two
// expect: 30
// expect: retried
// expect: 40
const str = @import("std/str");

fn risky(n: i64) !i64 {
    if (n == 1) { return error.One; }
    if (n == 2) { return error.Two; }
    return n * 10;
}

/// Handle `One`, pass everything else on.
fn only_one(n: i64) !i64 {
    return risky(n) catch |e| {
        if (e == error.One) {
            print("handled One");
            return 111;
        }
        return e;
    };
}

/// The same, one level up, so the re-raised error crosses two frames.
fn twice(n: i64) !i64 {
    return only_one(n) catch |e| { return e; };
}

fn main() i64 {
    var i = 0;
    while (i < 4) : (i += 1) {
        const r = twice(i) catch |e| {
            if (e == error.Two) {
                print("outer caught Two");
            } else {
                print("outer caught something else");
            }
            continue;
        };
        print_int(r);
    }

    // The shape this is actually for: retry what is retryable and pass on what
    // is not, without writing the caller's whole error set out again.
    print(str.concat("retr", "ied"));
    print_int(retrying(4) catch return 1);
    return 0;
}

fn retrying(n: i64) !i64 {
    return risky(n) catch |e| {
        if (e == error.Two) { return risky(n + 1); }
        return e;
    };
}
