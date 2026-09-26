// Dynamically loaded C functions. The annotation is the native ABI contract:
//   const abs: fn(i32) i32 = try ffi.bind(lib, "abs");
//   print(abs(i32(-42)));
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

/// The buffer's native address, for a C function that takes a pointer.
/// `error.IoFailed` once it has been freed. Nothing checks what C does with
/// the address afterwards: that is the one part of this module the two calls
/// below cannot bound for you.
pub fn address(buffer: Buffer) !u64 { return try raw_address(buffer.handle); }

/// `size` bytes from `offset`, copied out. An offset and a *count*, not a
/// range, so the region read is `buffer[offset..offset+size]`.
///
/// Bounds-checked against the size the buffer was made with, and answers
/// rather than reading past it: `error.BadFormat` for a region that does not
/// fit -- which covers a negative offset, a negative size and a sum that
/// overflows -- and `error.IoFailed` for a buffer that has been freed. So a
/// caller need not do the arithmetic defensively, and a mistake is a
/// diagnostic rather than somebody else's memory.
pub fn read(buffer: Buffer, offset: i64, size: i64) !str {
    return try raw_read(buffer.handle, offset, size);
}

/// Copy `bytes` into the buffer at `offset`, writing exactly its length and no
/// terminator. Checked exactly as `read` is, and with the same two errors.
pub fn write(buffer: Buffer, offset: i64, bytes: str) !void {
    return raw_write(buffer.handle, offset, bytes);
}

/// Release the buffer. Idempotent, and every call above answers
/// `error.IoFailed` afterwards rather than reaching freed memory -- but an
/// address already handed to C is a dangling pointer from this moment, which
/// is the coordination `buffer` asks the caller for.
pub fn free(buffer: Buffer) void { raw_free(buffer.handle); return; }

/// Copy size bytes from a foreign pointer. The caller must guarantee it names
/// readable native memory for the entire copy. No W# heap address is valid.
pub fn read_pointer(address: u64, size: i64) !str { return try raw_read_pointer(address, size); }
