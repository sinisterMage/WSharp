// `https://`, end to end, against a server this library is also the author of.
//
// This is the line item 8 left open. It said `https://` was
// `error.NotSupported` "rather than a connection that quietly speaks the wrong
// protocol", and that removing it was what item 10 was for; this is the case
// that shows it removed. A URL goes in, a TLS 1.3 connection is made, a
// certificate chain is checked against an anchor the caller supplied, the name
// is checked against the certificate, and an ordinary HTTP request and
// response cross the encrypted stream.
//
// Nothing in `std/http` above the four socket calls knows any of that
// happened. The status mapping, the header parser and the body decoder are the
// same code the plaintext case runs, which is what "a TLS connection is a
// `Socket` by another name" was supposed to mean.
//
// The anchor is the fixture root rather than the machine's store, so the case
// says the same thing on a machine with no certificates installed. The
// system store is what `x509_roots.ws` is for.
//
// Port 0 and a worker, as `tls_socket.ws` does: a handshake needs both ends
// running at once, and a hardcoded port makes a test that fails whenever the
// machine happens to be using it.
// expect: 200
// expect: text/plain
// expect: hello over tls
// expect: /hello
const http = @import("std/http");
const tls = @import("std/tls");
const x509 = @import("std/x509");
const net = @import("std/net");
const text = @import("std/str");
const bytes = @import("std/bytes");
const server = @import("./modules/httpsserver.ws");

const ROOT = "308201073081baa00302010202010a300506032b6570301b3119301706035504030c10777368617270207465737420726f6f74301e170d3230303130313030303030305a170d3439313233313233353935395a301b3119301706035504030c10777368617270207465737420726f6f74302a300506032b65700321008139770ea87d175f56a35466c34c7ecccb8d8a91b4ee37a25df60f5b8fc9b394a3233021300f0603551d130101ff040530030101ff300e0603551d0f0101ff040403020106300506032b65700341009b401f03050c341bde47090e5667f494535dbc0da7128718f37a76cd0974525c9a3adb7c736807315b10a642d7d625428ba7b1039db25154f1329959d6c6e10a";

fn main() i64 {
    const l = net.listen("localhost", 0, 4) catch return 1;
    const port = net.local_port(l) catch return 2;
    const w = @spawn(server, l.handle) catch return 3;

    const roots = x509.parse_all([][]u8{ hex(ROOT) });
    var url = "https://localhost:";
    url = text.concat(url, text.from_int(port));
    url = text.concat(url, "/hello");

    const answer = http.request_with(url, "GET", "",
                                     tls.roots_config("localhost", roots)) catch return 4;
    print_int(answer.code);
    print(http.header(answer.headers, "content-type") orelse "?");
    print(answer.body);

    print(w.note() catch "the worker did not answer");
    @join(w) catch return 5;
    net.close_listener(l);
    return 0;
}

fn hex(s: str) []u8 { return bytes.from_hex(s) catch bytes.new(0); }
