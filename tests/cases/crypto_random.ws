// The system's generator, and the wall clock.
//
// What can be asserted about randomness in a test that must pass every time is
// narrow, and pretending otherwise would be worse than saying so: the length
// is what was asked for, two calls differ, and the bytes are not all the same
// one. That last check would fail once in 256^31 on a working generator and
// every time on one that returns a constant, which is the failure worth
// catching.
// expect: 32
// expect: differ
// expect: varied
// expect: empty
// expect: after 2020
const crypto = @import("std/crypto");
const time = @import("std/time");
const bytes = @import("std/bytes");
const array = @import("std/array");

fn main() i64 {
    const key = crypto.random(32) catch return 1;
    print_int(array.len(key));

    const nonce = crypto.random(12) catch return 2;
    const again = crypto.random(12) catch return 3;
    if (!bytes.equal(nonce, again)) { print("differ"); }

    var same = true;
    var i = 1;
    while (i < array.len(key)) : (i += 1) {
        if (key[i] != key[0]) { same = false; }
    }
    if (!same) { print("varied"); }

    // Asking for nothing is not an error.
    const none = crypto.random(0) catch return 4;
    if (array.len(none) == 0) { print("empty"); }

    // The clock, checked the only way a clock can be: against a date that has
    // certainly passed. 1577836800 is 2020-01-01.
    if (time.now() > 1577836800) { print("after 2020"); }
    return 0;
}
