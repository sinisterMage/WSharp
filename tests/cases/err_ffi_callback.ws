// error: FFI binding requires a C function signature
const ffi = @import("std/ffi");
fn main() void {
    const bad: fn(fn(i32) i32) void = ffi.bind(ffi.Library{ .handle = 0 }, "bad") catch return;
    return;
}
