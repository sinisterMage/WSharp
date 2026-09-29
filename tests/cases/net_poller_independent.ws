// One thread waiting on its poller must not stop another using its own.
//
// Every poller lives in one process-wide table, and `net.wait` used to hold
// that table's lock for its whole timeout -- so while a worker waited, the
// main thread's `net.poller()` (and `watch`, `forget`, `wait` and the result
// readers, on any poller) waited with it, outside a safe region. With a
// timeout of -1 that is a hang with no diagnostic. Found by Raython, whose
// acceptors took turns on their pollers instead of waiting together.
//
// The worker waits up to 30 s and is woken by a write, so the case is fast
// when it passes; before the fix `net.poller()` below took the better part of
// the 30 s and the first line said so.
// expect: independent
// expect: woken
const net = @import("std/net");
const time = @import("std/time");
const waiter = @import("./modules/pollwaiter.ws");

fn main() i64 {
    const listener = net.listen("127.0.0.1", 0, 4) catch return 1;
    const port = net.local_port(listener) catch return 2;
    const w = @spawn(waiter, port, 30000) catch return 3;
    const conn = net.accept(listener) catch return 4;
    // Long enough for the worker to be inside its wait. Arriving early makes
    // the case pass for the wrong reason, never fail for one.
    time.sleep_ms(1000) catch return 5;

    const t0 = time.monotonic_ms();
    const p = net.poller() catch return 6;
    const took = time.monotonic_ms() - t0;
    net.close_poller(p);
    if (took < 10000) { print("independent"); } else { print("blocked"); }

    net.write_all(conn, "x") catch return 7;
    const waited = w.waited() catch return 8;
    @join(w) catch return 9;
    if (waited >= 0 and waited < 25000) { print("woken"); } else { print_int(waited); }
    net.close(conn);
    net.close_listener(listener);
    return 0;
}
