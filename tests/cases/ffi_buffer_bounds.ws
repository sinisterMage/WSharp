// `std/ffi`'s buffer accessors are checked, and answer rather than reaching.
//
// The clause the module now states, given a case. `read` and `write` name a
// region as an offset and a count, so every way of getting that arithmetic
// wrong -- past the end, a negative offset, a negative size -- has to be an
// answer and not a read or a write into somebody else's memory. `BadFormat` is
// a region that does not fit; `IoFailed` is a buffer that is gone.
//
// Worth a case rather than a sentence because this is the one module where the
// failure mode is not a wrong number: an unchecked `write` past the end of a
// native allocation corrupts whatever the allocator put beside it, and the
// program that notices is some other one. A caller who cannot tell whether
// these are checked has to do the arithmetic again itself, which is a second
// place to get it wrong.
//
// No library is opened, so this runs wherever the suite does.
//
// expect: in bounds
// expect: whole buffer read back
// expect: overrun write refused
// expect: overrun read refused
// expect: negative offset refused
// expect: negative size refused
// expect: read after free refused
// expect: write after free refused
// expect: address after free refused
// expect: freeing twice is fine
const ffi = @import("std/ffi");
const text = @import("std/str");

// What a call answered, as a number, because an error is not a value here.
const OK = 0;
const BAD_FORMAT = 1;
const IO_FAILED = 2;
const OTHER = 3;

fn main() i64 {
    const b = ffi.buffer(4) catch return 1;

    if (wrote(b, 0, "abcd") != OK) { return 2; }
    print("in bounds");
    const whole = ffi.read(b, 0, 4) catch return 3;
    if (!text.eq(whole, "abcd")) { return 4; }
    print("whole buffer read back");

    // One byte more than the buffer holds, from an offset that is itself fine.
    if (wrote(b, 2, "xyz") != BAD_FORMAT) { return 5; }
    print("overrun write refused");
    if (red(b, 2, 8) != BAD_FORMAT) { return 6; }
    print("overrun read refused");

    // Negative in each position. Neither is read as a large unsigned number.
    if (red(b, -1, 2) != BAD_FORMAT) { return 7; }
    print("negative offset refused");
    if (red(b, 0, -1) != BAD_FORMAT) { return 8; }
    print("negative size refused");

    // And once the buffer is gone every door is shut rather than left open.
    ffi.free(b);
    if (red(b, 0, 1) != IO_FAILED) { return 9; }
    print("read after free refused");
    if (wrote(b, 0, "a") != IO_FAILED) { return 10; }
    print("write after free refused");
    const addr = ffi.address(b) catch |e| {
        if (e != error.IoFailed) { return 11; }
        print("address after free refused");
        u64(0)
    };
    if (addr != 0) { return 12; }

    ffi.free(b);
    print("freeing twice is fine");
    return 0;
}

/// Which of the two errors a read answered with, or `OK`.
fn red(b: ffi.Buffer, at: i64, n: i64) i64 {
    const got = ffi.read(b, at, n) catch |e| {
        if (e == error.BadFormat) { return BAD_FORMAT; }
        if (e == error.IoFailed) { return IO_FAILED; }
        return OTHER;
    };
    return OK;
}

fn wrote(b: ffi.Buffer, at: i64, s: str) i64 {
    ffi.write(b, at, s) catch |e| {
        if (e == error.BadFormat) { return BAD_FORMAT; }
        if (e == error.IoFailed) { return IO_FAILED; }
        return OTHER;
    };
    return OK;
}
