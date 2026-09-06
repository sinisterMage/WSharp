// One thread serving two connections, by asking which of them has said
// something rather than by blocking on each in turn.
//
// The loop is the point, and it is not defensive padding. A readiness API is
// allowed to report a *subset* of what is ready, and platforms differ in what
// they report and when: Linux completes a loopback write inside the syscall, so
// both connections are ready at once, while macOS hands loopback delivery to
// the kernel and a wait issued immediately afterwards may see one of them, or
// neither. A case that assumed one wait would report both passed everywhere and
// then failed on macOS under `--gc-stress`, which shifted the timing just
// enough. Waiting until each socket has actually been served is the shape a
// real server has anyway.
// expect: served both
// expect: from a
// expect: from b
const net = @import("std/net");
const list = @import("std/list");
const text = @import("std/str");

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

    net.write_all(a, "from a") catch return 10;
    net.write_all(b, "from b") catch return 11;

    var from_a = "";
    var from_b = "";
    var rounds = 0;
    // Bounded so that a poller reporting nothing for ever fails the case
    // rather than hanging the suite.
    while ((text.len(from_a) == 0 or text.len(from_b) == 0) and rounds < 100) : (rounds += 1) {
        const ready = net.wait(p, 5000) catch return 12;
        var i = 0;
        while (i < list.len(ready)) : (i += 1) {
            const event = list.get(ready, i);
            // Read only what has not been read: a socket already drained is
            // not reported again, but asking twice would block for ever if it
            // were.
            if (event.socket.handle == sa.handle and text.len(from_a) == 0) {
                from_a = net.read_exactly(sa, 6) catch return 13;
            }
            if (event.socket.handle == sb.handle and text.len(from_b) == 0) {
                from_b = net.read_exactly(sb, 6) catch return 14;
            }
        }
    }
    if (text.len(from_a) == 0 or text.len(from_b) == 0) { return 15; }
    print("served both");
    print(from_a);
    print(from_b);

    net.forget(p, sa) catch return 16;
    net.close_poller(p);
    net.close(a);
    net.close(b);
    net.close(sa);
    net.close(sb);
    net.close_listener(l);
    return 0;
}
