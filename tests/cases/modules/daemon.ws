// Two workers whose `init` is the work, rather than a thing that returns state.
//
// `@spawn` hands the handle back before `init` starts, so a module whose `init`
// never returns is a self-driving daemon -- which is what makes N acceptors on
// one listener a two-line program. The price is that such a worker never
// reaches its message loop and so can never be told to stop, which is what
// `main` returning has to answer for.
const net = @import("std/net");

pub const State = struct { served: i64 };

/// Accept one connection, read what is on it, and become an ordinary worker.
///
/// The blocking call is the point: it goes through the runtime's safe region,
/// and until the argument pins were dropped before entering generated code it
/// parked with runtime roots on the thread -- an abort in any build with debug
/// assertions on, for the one program shape `@spawn` exists for.
pub fn init(l: net.Listener, greeting: str) State {
    const s = net.accept(l) catch return State{ .served = -1 };
    // Reads to end-of-stream, which is what the client's `shutdown` produced.
    const said = net.read_all(s) catch "";
    net.close(s);
    print(greeting);
    print(said);
    return State{ .served = 1 };
}

pub fn served(s: State) i64 { return s.served; }
