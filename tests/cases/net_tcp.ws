// A TCP conversation with itself, over the loopback interface.
//
// Single-threaded on purpose: `connect` to a listening socket completes into
// the backlog without anyone having called `accept` yet, so one thread can be
// both ends and the case cannot deadlock in CI.
//
// Port 0 asks the kernel for a free port and `local_port` reports which -- a
// hardcoded port would make a test that fails whenever the machine happens to
// be using it.
// expect: bound
// expect: ping
// expect: pong
// expect: gone
const net = @import("std/net");

fn main() i64 {
    const l = net.listen("127.0.0.1", 0, 4) catch return 1;
    const port = net.local_port(l) catch return 2;
    if (port <= 0) { return 3; }
    print("bound");

    const client = net.connect("127.0.0.1", port) catch return 4;
    const served = net.accept(l) catch return 5;

    net.write_all(client, "ping") catch return 6;
    print(net.read_exactly(served, 4) catch return 7);

    net.write_all(served, "pong") catch return 8;
    print(net.read_exactly(client, 4) catch return 9);

    net.close(client);
    net.close(served);
    net.close_listener(l);

    // A handle that names nothing is an error a program can catch rather than
    // a crash, which is what lets `close` be safe to call twice.
    print(net.read(served, 1) catch "gone");
    return 0;
}
