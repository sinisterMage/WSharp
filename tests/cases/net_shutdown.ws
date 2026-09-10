// expect: the server read: GET / HTTP/1.0
// expect: the client read: 200 OK
// expect: shutting down a closed socket is an error, not a panic
// A half-close: one direction shut down, the other still open.
//
// What `close` cannot say. A client that has finished sending but still wants
// the answer shuts down its write half; the peer reads end-of-stream and knows
// the request is whole, while the reply still has somewhere to go. Every
// protocol that does not frame its own end needs exactly this, and until now
// `std/net` had no way to spell it -- a server written against `read_all` could
// only be talked to by a client that hung up entirely.
//
// Single-threaded, as every networking case here is: `connect` to a listening
// socket completes into the backlog with nobody in `accept`, so one thread is
// both ends and the case cannot deadlock. Port 0, and then ask what was given.
//
// A `Listener` is deliberately not shut down anywhere here. `shutdown` on one
// wakes a thread parked in `accept` on Linux and answers `ENOTCONN` on the
// BSDs, so it is not a thing this library promises; a stoppable acceptor is a
// `poller` with a tick.
const net = @import("std/net");
const text = @import("std/str");

fn main() i64 {
    const l = net.listen("127.0.0.1", 0, 8) catch return 1;
    const port = net.local_port(l) catch return 2;

    const client = net.connect("127.0.0.1", port) catch return 3;
    net.write_all(client, "GET / HTTP/1.0") catch return 4;
    net.shutdown(client, false, true) catch return 5;

    const server = net.accept(l) catch return 6;
    // Terminates, which it would not have without the shutdown above.
    print(text.concat("the server read: ", net.read_all(server) catch return 7));

    // The other direction is still open, which is the whole point.
    net.write_all(server, "200 OK") catch return 8;
    net.shutdown(server, false, true) catch return 9;
    print(text.concat("the client read: ", net.read_all(client) catch return 10));

    net.close(server);
    net.close(client);
    net.close_listener(l);

    // A handle that names nothing is an error to catch rather than a panic,
    // which is `close`'s rule read the other way round.
    net.shutdown(server, true, true) catch {
        print("shutting down a closed socket is an error, not a panic");
        return 0;
    };
    return 11;
}
