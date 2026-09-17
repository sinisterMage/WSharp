// expect: hello
// expect: -128
// expect: -32768
// expect: -2147483648
// expect: -9223372036854775808
// expect: 255
// expect: 65535
// expect: 4294967295
// expect: 18446744073709551615
// expect: 42
// expect: 18446744073709551615
// expect: 1.0
// expect: 1.5
// expect: true
// expect: false
// expect: generic
// expect: -7
// expect: 2.5
// expect: local
// expect: 9
// expect: evaluated once
// expect: 3

fn show(value) void { print(value); }
fn forward(value) void { show(value); }
fn number(value: Number) void { print(value); }
fn once() i16 { print("evaluated once"); return 3; }

fn main() void {
    print("hello");
    print(i8(-128));
    print(i16(-32768));
    print(i32(-2147483648));
    print(i64(-9223372036854775808));
    print(u8(255));
    print(u16(65535));
    print(u32(4294967295));
    print(u64(18446744073709551615));
    print(42);
    print(18446744073709551615);
    print(1.0);
    print(1.5);
    print(true);
    print(false);
    forward("generic");
    forward(i8(-7));
    number(2.5);
    const local = fn (value) void { print(value); };
    local("local");
    local(9);
    print(once());
}
