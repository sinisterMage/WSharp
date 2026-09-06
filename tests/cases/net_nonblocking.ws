// A non-blocking socket answers at once, and "nothing yet" is an error name
// rather than a wait.
// expect: would block
// expect: accepted
const net = @import("std/net");

/// Whether `accept` came straight back saying nobody had connected.
///
/// The `catch` leaves rather than producing a socket, which is what a block
/// with no value has to do -- and is why this is a function rather than an
/// expression in `main`.
fn nobody_yet(l: net.Listener) bool {
    const early = net.accept(l) catch |e| { return e == error.WouldBlock; };
    net.close(early);
    return false;
}

fn main() i64 {
    const l = net.listen("127.0.0.1", 0, 4) catch return 1;
    const port = net.local_port(l) catch return 2;
    net.accept_nonblocking(l, true) catch return 3;

    if (!nobody_yet(l)) { return 4; }
    print("would block");

    const client = net.connect("127.0.0.1", port) catch return 5;
    const served = net.accept(l) catch return 6;
    print("accepted");

    net.close(client);
    net.close(served);
    net.close_listener(l);
    return 0;
}
