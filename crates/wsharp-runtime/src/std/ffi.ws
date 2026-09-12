// Dynamically loaded C functions. The annotation is the native ABI contract:
//   const abs: fn(i32) i32 = try ffi.bind(lib, "abs");
//   print_int(i64(abs(i32(-42))));
//
// Integers keep their widths, bool is C _Bool, f64 is C double, and u64 carries
// a native pointer on W#'s 64-bit targets. Return void for a C void function.
// Structs by value, variadic functions, callbacks and W# heap references are
// not supported. A wrong native signature is undefined behaviour.

pub const Library = struct { handle: i64 };
pub const Buffer = struct { handle: i64 };

/// Load a .so, .dylib or .dll using the operating system's search rules.
/// Prefer an explicit path. `last_error()` supplies the loader's explanation.
pub fn open(path: str) !Library { return Library{ .handle = try raw_open(path) }; }

/// Bind a symbol to the function type supplied by the surrounding annotation.
/// Its signature must exactly match C, including signedness and integer width.
/// A binding stays valid until close(library); calling it afterwards panics.
pub fn bind[T](library: Library, name: str) !T {
    const bound: T = raw_bind(try raw_symbol(library.handle, name));
    return bound;
}

/// Invalidate this library's bindings and release it after active calls return.
pub fn close(library: Library) void { raw_close(library.handle); return; }

/// Zeroed, stable native memory. Release explicitly with free().
/// Native code may retain its address until free; callers coordinate access.
pub fn buffer(size: i64) !Buffer { return Buffer{ .handle = try raw_buffer(size) }; }

/// Copy bytes into native storage, adding one NUL terminator.
/// Embedded NULs are rejected, since C would see only the prefix.
pub fn c_string(value: str) !Buffer { return Buffer{ .handle = try raw_c_string(value) }; }

pub fn address(buffer: Buffer) !u64 { return try raw_address(buffer.handle); }
pub fn read(buffer: Buffer, offset: i64, size: i64) !str {
    return try raw_read(buffer.handle, offset, size);
}
pub fn write(buffer: Buffer, offset: i64, bytes: str) !void {
    return raw_write(buffer.handle, offset, bytes);
}
pub fn free(buffer: Buffer) void { raw_free(buffer.handle); return; }

/// Copy size bytes from a foreign pointer. The caller must guarantee it names
/// readable native memory for the entire copy. No W# heap address is valid.
pub fn read_pointer(address: u64, size: i64) !str { return try raw_read_pointer(address, size); }
