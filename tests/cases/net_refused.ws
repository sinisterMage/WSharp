// Connecting where nothing is listening has a name a program can act on.
//
// The port is one that was free a moment ago: bind, ask which, then give it
// up. Picking a number instead would be a test that fails when something else
// happens to be listening on it.
// expect: refused
const net = @import("std/net");

fn main() i64 {
    const l = net.listen("127.0.0.1", 0, 1) catch return 1;
    const port = net.local_port(l) catch return 2;
    net.close_listener(l);

    net.connect("127.0.0.1", port) catch |e| {
        if (e == error.ConnectionRefused) { print("refused"); } else { print("wrong error"); }
        return 0;
    };
    print("connected to nothing");
    return 3;
}
