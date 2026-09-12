// error: FFI binding requires a C function signature
const ffi = @import("std/ffi");
fn main() void {
    const bad: fn() []u8 = ffi.bind(ffi.Library{ .handle = 0 }, "bad") catch return;
    return;
}
