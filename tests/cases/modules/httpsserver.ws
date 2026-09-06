// An HTTPS server on a worker of its own, for `https_loopback.ws`.
//
// The whole of it is in `init`, for `tlsecho.ws`'s reason: `@spawn` hands back
// a handle without waiting for `init`, where a method call would block the
// caller -- and a caller blocked in `w.serve()` never sends the request the
// server is waiting for.
//
// It sends the leaf and the intermediate and not the root, which is what a
// server is supposed to do: the anchor is the client's, and a server that sent
// its own would be asking to be trusted on its own say-so.
const tls = @import("std/tls");
const http = @import("std/http");
const net = @import("std/net");
const bytes = @import("std/bytes");

const SEED = "0101010101010101010101010101010101010101010101010101010101010101";
const LEAF = "308201313081e4a00302010202011e300506032b657030233121301f06035504030c18777368617270207465737420696e7465726d656469617465301e170d3230303130313030303030305a170d3439313233313233353935395a301d311b301906035504030c12777368617270207465737420736572766572302a300506032b65700321008a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5ca3433041300c0603551d130101ff04023000300e0603551d0f0101ff04040302078030210603551d11041a301882096c6f63616c686f7374820b2a2e77696c642e74657374300506032b65700341005b8b486c3336e4a16d5a24e3998189ee9277f8860e1a555d52f067ca2ec1082376b68b484969c062b76b217fad5b028b5f6517be84e3c0b4537745a8e8ac850c";
const INTER = "308201123081c5a003020102020114300506032b6570301b3119301706035504030c10777368617270207465737420726f6f74301e170d3230303130313030303030305a170d3439313233313233353935395a30233121301f06035504030c18777368617270207465737420696e7465726d656469617465302a300506032b6570032100ca93ac1705187071d67b83c7ff0efe8108e8ec4530575d7726879333dbdabe7ca326302430120603551d130101ff040830060101ff020100300e0603551d0f0101ff040403020106300506032b657003410052f9b98960d7f5552aa04136091af3eec6bd04368c24cbfc44e8e3c53faa12e46f6e1dd8046bffbd4854198dd3085ecd9aea1b64cb84cfc29005cb4f3c3f940f";

pub const State = struct { note: str };

pub fn init(listener: i64) State {
    return State{ .note = serve(listener) };
}

fn serve(listener: i64) str {
    const l = net.Listener{ .handle = listener };
    const raw = net.accept(l) catch return "the accept failed";
    const scfg = tls.server_config([][]u8{ hex(LEAF), hex(INTER) }, hex(SEED));
    const s = tls.accept(raw, scfg) catch return "the handshake failed";
    // The same `Conn` the plaintext server uses, with a session in it.
    const c = http.tls_connection(s);
    const request = http.read_request(c) catch return "the request failed";
    http.respond(c, 200, "text/plain", "hello over tls") catch return "no response";
    http.close(c);
    return path_of(request);
}

fn path_of(r: http.Request) str {
    return bytes.to_str(bytes.of(r.path));
}

pub fn note(s: State) str { return s.note; }

fn hex(s: str) []u8 { return bytes.from_hex(s) catch bytes.new(0); }
