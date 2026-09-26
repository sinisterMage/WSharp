// There is no way to shut down a listener, and the compiler says so.
//
// This is the guard for LIMITATIONS.md, "`net.shutdown` does not stop an
// acceptor on the BSDs". The limitation is usually described as a difference
// between operating systems -- Linux wakes a thread parked in `accept`, the BSDs
// answer `ENOTCONN` and leave it parked -- but the thing a *program* meets is
// one step earlier and is the same on every platform: `net.shutdown` takes a
// `Socket`, a `Listener` is a different type, and there is no
// `shutdown_listener`. So the divergence is not reachable through the API at
// all, and a type error is what a user gets.
//
// Asserting the type error rather than the platform behaviour is deliberate, and
// it is `fs_chmod.ws`'s discipline: a case that said "Linux succeeds and the
// others raise" would be a case about the platforms it was written on, and the
// four release triples include two Darwin targets and one Windows target whose
// answers are not the same fact. What is true everywhere is that the API does
// not offer this, and that is what must not drift -- if a `shutdown_listener`
// is ever added, this case fails and the LIMITATIONS.md entry gets read again.
//
// The stoppable acceptor is `net_poller.ws`: `accept_nonblocking`,
// `watch_listener` and `wait(p, ms)`, with the stop signal coming from whatever
// the program already has.
// error: type mismatch
// error: Listener
// error: Socket
const net = @import("std/net");

fn main() i64 {
    const l = net.listen("127.0.0.1", 0, 8) catch return 1;
    net.shutdown(l, true, true) catch return 2;
    net.close_listener(l);
    return 0;
}
