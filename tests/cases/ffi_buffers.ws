// expect: ffi buffers ok
const ffi = @import("std/ffi");
const text = @import("std/str");
fn main() i64 {
    ffi.open("/wsharp/does-not-exist/library") catch |e| {
        assert(e == error.NotFound);
        assert(text.len(ffi.last_error()) > 0);
        ffi.Library{ .handle = 0 }
    };
    ffi.open("bad\0name") catch |e| {
        assert(e == error.BadFormat);
        ffi.Library{ .handle = 0 }
    };
    const b = ffi.buffer(4) catch return 1;
    assert((ffi.read(b, 0, 4) catch return 2) == "\0\0\0\0");
    ffi.write(b, 1, "abc") catch return 2;
    assert((ffi.read(b, 0, 4) catch return 2) == "\0abc");
    ffi.write(b, 2, "abc") catch |e| assert(e == error.BadFormat);
    assert((ffi.read(b, -1, 1) catch |e| { assert(e == error.BadFormat); "bounds" }) == "bounds");
    assert((ffi.read(b, 0, 9223372036854775807) catch |e| { assert(e == error.BadFormat); "bounds" }) == "bounds");
    ffi.free(b);
    ffi.free(b);
    assert((ffi.address(b) catch |e| { assert(e == error.IoFailed); u64(0) }) == u64(0));
    const replacement = ffi.buffer(4) catch return 2;
    assert((ffi.address(b) catch u64(0)) == u64(0));
    ffi.free(replacement);
    assert((ffi.read_pointer(0, 0) catch return 2) == "");
    assert((ffi.read_pointer(0, 1) catch |e| { assert(e == error.BadFormat); "null" }) == "null");
    ffi.c_string("bad\0value") catch |e| { assert(e == error.BadFormat); ffi.Buffer{ .handle = 0 } };
    ffi.buffer(-1) catch |e| { assert(e == error.BadFormat); ffi.Buffer{ .handle = 0 } };
    print("ffi buffers ok");
    return 0;
}
