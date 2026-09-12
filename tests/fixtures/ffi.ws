const ffi = @import("std/ffi");
const os = @import("std/os");
const text = @import("std/str");

fn main() i64 {
    const library = ffi.open(os.args()[0]) catch { print(ffi.last_error()); return 1; };
    const answer: fn() i32 = ffi.bind(library, "answer") catch return 2;
    assert(answer() == i32(42));
    const neg: fn(i8) i8 = ffi.bind(library, "negate8") catch return 2;
    assert(neg(i8(-42)) == i8(42));
    assert(neg(i8(42)) == i8(-42));
    const u8fn: fn(u8) u8 = ffi.bind(library, "echo8") catch return 2;
    assert(u8fn(u8(255)) == u8(255));
    const i16fn: fn(i16) i16 = ffi.bind(library, "echo16") catch return 2;
    assert(i16fn(i16(-30000)) == i16(-30000));
    const u16fn: fn(u16) u16 = ffi.bind(library, "echou16") catch return 2;
    assert(u16fn(u16(60000)) == u16(60000));
    const i32fn: fn(i32) i32 = ffi.bind(library, "echo32") catch return 2;
    assert(i32fn(i32(-2000000000)) == i32(-2000000000));
    const u32fn: fn(u32) u32 = ffi.bind(library, "echou32") catch return 2;
    assert(u32fn(u32(4000000000)) == u32(4000000000));
    const i64fn: fn(i64) i64 = ffi.bind(library, "echo64") catch return 2;
    assert(i64fn(-5000000000) == -5000000000);
    const u64fn: fn(u64) u64 = ffi.bind(library, "echou64") catch return 2;
    assert(u64fn(18446744073709551615) == u64(18446744073709551615));
    const invert: fn(bool) bool = ffi.bind(library, "invert") catch return 2;
    assert(invert(false));
    assert(!invert(true));
    const many_ints: fn(i64, i64, i64, i64, i64, i64, i64, i64, i64) i64 =
        ffi.bind(library, "many_ints") catch return 2;
    assert(many_ints(1, 2, 3, 4, 5, 6, 7, 8, 9) == 45);
    const many_floats: fn(f64, f64, f64, f64, f64, f64, f64, f64, f64, f64) f64 =
        ffi.bind(library, "many_floats") catch return 2;
    assert(many_floats(1.5, 2.5, 3.5, 4.5, 5.5, 6.5, 7.5, 8.5, 9.5, 10.5) == 60.0);
    const mixed: fn(i8, f64, u32, f64, i64, f64, i32, f64, i16, f64) f64 =
        ffi.bind(library, "mixed") catch return 2;
    assert(mixed(i8(-1), 2.5, u32(3), 4.5, 5, 6.5, i32(7), 8.5, i16(-9), 10.5) == 37.5);

    const length: fn(u64) u64 = ffi.bind(library, "length") catch return 2;
    const value = ffi.c_string(text.concat("hello", " native")) catch return 3;
    const pointer = ffi.address(value) catch return 3;
    gc_trace();
    assert(length(pointer) == u64(12));
    const fill: fn(u64, u64, u8) void = ffi.bind(library, "fill") catch return 2;
    fill(pointer, 5, u8(120));
    assert((ffi.read(value, 0, 12) catch return 3) == "xxxxx native");
    ffi.write(value, 0, "hello") catch return 3;
    assert((ffi.read_pointer(pointer, 12) catch return 3) == "hello native");
    const greeting: fn() u64 = ffi.bind(library, "greeting") catch return 2;
    assert((ffi.read_pointer(greeting(), 6) catch return 3) == "native");

    const pause: fn(i32) void = ffi.bind(library, "pause_ms") catch return 2;
    const live = text.concat("alive", " across C");
    gc_trace_start();
    pause(i32(25));
    gc_trace_finish();
    assert(live == "alive across C");
    assert(answer() == i32(42));
    const missing: fn() void = ffi.bind(library, "missing_symbol") catch |e| {
        assert(e == error.NotFound);
        assert(text.len(ffi.last_error()) > 0);
        fn() void { return; }
    };
    ffi.free(value);
    ffi.close(library);
    if (os.args()[1] == "closed") { answer(); }
    print("ffi ok");
    return 0;
}
