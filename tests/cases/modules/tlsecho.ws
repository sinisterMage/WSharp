// A TLS server on a worker of its own, for `tls_socket.ws`.
//
// The whole of it happens in `init`, which is the one thing a service can do
// *concurrently* with its spawner: `@spawn` starts the thread and hands back a
// handle without waiting for `init` to finish, while an ordinary method call
// blocks the caller until the worker answers it. A TLS handshake needs both
// ends running at once, so `init` is where the server has to live.
//
// The listener is passed as its raw handle. A socket belongs to the process
// rather than to any one worker's heap -- it is an index into a table, exactly
// as a broker topic is -- so a number is the whole of what has to cross.
const tls = @import("std/tls");
const net = @import("std/net");
const bytes = @import("std/bytes");
const array = @import("std/array");

const SEED = "0101010101010101010101010101010101010101010101010101010101010101";
const CERT = "308201283081dba003020102020101300506032b6570301b3119301706035504030c107773686172702074657374206c656166301e170d3230303130313030303030305a170d3439313233313233353935395a301b3119301706035504030c107773686172702074657374206c656166302a300506032b65700321008a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5ca3443042300c0603551d130101ff04023000300e0603551d0f0101ff04040302078030220603551d11041b301982096c6f63616c686f7374820c6578616d706c652e74657374300506032b6570034100c5b3b0183715a28a33bbe5abd60e5e2a2679d235022b24b93699b05ab88d470ea8330b598b989ddf84609e9560c610e1c996685314ad8519f6562515baf31401";

pub const State = struct { note: str };

pub fn init(listener: i64) State {
    return State{ .note = serve(listener) };
}

/// Accept one connection, echo one message, and say what happened.
///
/// Every failure becomes a sentence rather than an error, because `init` has
/// no way to raise one -- and a sentence read back through `note` is more use
/// in a test than a worker that quietly died.
fn serve(listener: i64) str {
    const l = net.Listener{ .handle = listener };
    const raw = net.accept(l) catch return "the accept failed";
    const scfg = tls.server_config([][]u8{ hex(CERT) }, hex(SEED));
    const s = tls.accept(raw, scfg) catch return "the handshake failed";
    const buf = bytes.new(4096);
    const n = tls.read(s, buf, 0, 4096) catch return "the read failed";
    tls.write_all(s, buf, 0, n) catch return "the write failed";
    tls.close_session(s);
    return "the server echoed one message";
}

pub fn note(s: State) str { return s.note; }

fn hex(s: str) []u8 { return bytes.from_hex(s) catch bytes.new(0); }
