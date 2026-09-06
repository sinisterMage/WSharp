// One thread serving two connections, by asking which of them has said
// something rather than by blocking on each in turn.
// expect: two ready
// expect: from a
// expect: from b
const net = @import("std/net");
const list = @import("std/list");

fn main() i64 {
    const l = net.listen("127.0.0.1", 0, 8) catch return 1;
    const port = net.local_port(l) catch return 2;

    const a = net.connect("127.0.0.1", port) catch return 3;
    const b = net.connect("127.0.0.1", port) catch return 4;
    const sa = net.accept(l) catch return 5;
    const sb = net.accept(l) catch return 6;

    const p = net.poller() catch return 7;
    net.watch(p, sa, true, false) catch return 8;
    net.watch(p, sb, true, false) catch return 9;

    // Both clients speak before anything waits, so both are ready at once and
    // the order the server reads them in is its own choice, not theirs.
    net.write_all(a, "from a") catch return 10;
    net.write_all(b, "from b") catch return 11;

    const ready = net.wait(p, 5000) catch return 12;
    if (list.len(ready) != 2) { return 13; }
    print("two ready");

    // Read in the order the sockets were accepted, not the order reported, so
    // the expectations do not depend on what the kernel happened to notice
    // first.
    print(net.read_exactly(sa, 6) catch return 14);
    print(net.read_exactly(sb, 6) catch return 15);

    net.forget(p, sa) catch return 16;
    net.close_poller(p);
    net.close(a);
    net.close(b);
    net.close(sa);
    net.close(sb);
    net.close_listener(l);
    return 0;
}
