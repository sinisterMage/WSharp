// A worker that connects to `port`, watches the connection on a poller of its
// own, and waits on it -- for at most `ms`, and in practice until the other
// end writes. How long it waited is its state.
const net = @import("std/net");
const time = @import("std/time");

pub const State = struct { waited: i64 };

pub fn init(port: i64, ms: i64) State {
    const s = net.connect("127.0.0.1", port) catch return State{ .waited = -1 };
    const p = net.poller() catch return State{ .waited = -2 };
    net.watch(p, s, true, false) catch return State{ .waited = -3 };
    const t0 = time.monotonic_ms();
    const ready = net.wait(p, ms) catch return State{ .waited = -4 };
    const took = time.monotonic_ms() - t0;
    net.close_poller(p);
    net.close(s);
    return State{ .waited = took };
}

pub fn waited(s: State) i64 { return s.waited; }
