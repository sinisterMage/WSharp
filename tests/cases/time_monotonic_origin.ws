// `time.monotonic_ms`'s origin is not the start of the process.
//
// The clause the module's documentation now states, given a case so that it is
// checked rather than asserted: only the *difference* between two readings is
// meaningful, and in particular a first reading says nothing about how long the
// program has already been running. `time_monotonic.ws` covers the difference;
// this covers the origin, which it cannot, because it reads the clock before it
// sleeps and so fixes the origin at the top of the program.
//
// The shape is the only one that can tell the two apart: spend a known amount
// of time *without* touching the clock, then read it for the first time. An
// origin at process start would have to answer at least that much; the origin
// this library promises answers about zero, because it is fixed by this very
// call. Bounded rather than exact, since the reading is a small positive number
// of milliseconds on a busy machine and zero on an idle one -- what is being
// checked is that it is not the sleep.
//
// expect: the origin is not process start
// expect: and the difference still measures
const time = @import("std/time");

fn main() i64 {
    const slept = 120;
    time.sleep_ms(slept) catch return 1;

    // The first reading in the process, after 120 ms of provable elapsed time.
    const first = time.monotonic_ms();
    if (first < 0) { return 2; }
    if (first >= slept) { return 3; }
    print("the origin is not process start");

    // And the clock is still a clock: two readings around a sleep differ by it.
    time.sleep_ms(30) catch return 4;
    const second = time.monotonic_ms();
    if (second - first < 25) { return 5; }
    print("and the difference still measures");
    return 0;
}
