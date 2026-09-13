// expect: monotonic clock and sleep
const time = @import("std/time");
fn main() i64 {
    const a = time.monotonic_ms();
    time.sleep_ms(12) catch return 1;
    const b = time.monotonic_ms();
    if (b < a + 10) { return 2; }
    time.sleep_ms(-1) catch |e| {
        if (e != error.BadFormat) { return 3; }
        print("monotonic clock and sleep"); return 0;
    };
    return 4;
}
