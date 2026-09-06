// The blocking wrapper, over a real socket, with a real second thread.
//
// Everything else about TLS in this tree is tested on buffers, because a
// handshake is a negotiation and one thread cannot be both ends of one. This
// is the case that proves the layer that does touch a socket: `tls.connect`,
// `tls.accept`, `tls.read`, `tls.write_all` and the close_notify at the end.
//
// The server runs on a worker, and its whole job is in that service's `init`
// -- which is the only part of a worker that runs *while* its spawner
// continues. A method call would block the caller, and a caller blocked in
// `w.serve()` is a caller that will never send the ClientHello the server is
// waiting for.
//
// Port 0, as every networking case here does: a hardcoded one makes a test
// that fails whenever the machine happens to be using it.
//
// This also exercises the collector's safe region twice over. Both ends block
// in `read(2)` inside `worker::blocking`, on two threads with two heaps, while
// holding a session object full of keys -- which is exactly the shape the safe
// region exists for.
// expect: over a real socket
// expect: the server echoed one message
const tls = @import("std/tls");
const x509 = @import("std/x509");
const net = @import("std/net");
const bytes = @import("std/bytes");
const array = @import("std/array");
const echo = @import("./modules/tlsecho.ws");

const SPKI = "302a300506032b65700321008a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c";

fn main() i64 {
    const l = net.listen("127.0.0.1", 0, 4) catch return 1;
    const port = net.local_port(l) catch return 2;
    const w = @spawn(echo, l.handle) catch return 3;

    const raw = net.connect("127.0.0.1", port) catch return 4;
    const key = x509.parse_spki(hex(SPKI)) catch return 5;
    const s = tls.connect(raw, tls.pinned_config("localhost", key)) catch return 6;

    const msg = bytes.of("over a real socket");
    tls.write_all(s, msg, 0, array.len(msg)) catch return 7;
    const buf = bytes.new(256);
    const n = tls.read(s, buf, 0, 256) catch return 8;
    print(bytes.slice_str(buf, 0, n));
    tls.close_session(s);

    print(w.note() catch "the worker did not answer");
    @join(w) catch return 9;
    net.close_listener(l);
    return 0;
}

fn hex(s: str) []u8 { return bytes.from_hex(s) catch bytes.new(0); }
