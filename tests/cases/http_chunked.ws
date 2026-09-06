// A body sent in pieces, each announced by its length in hexadecimal.
//
// The response is written by hand rather than by `respond`, because `respond`
// always declares a length -- chunked is what a *server elsewhere* does, and
// this is the client half being able to read it.
// expect: hello, world
// expect: 12
const net = @import("std/net");
const http = @import("std/http");

fn main() i64 {
    const l = net.listen("127.0.0.1", 0, 4) catch return 1;
    const port = net.local_port(l) catch return 2;
    const client = http.connection(net.connect("127.0.0.1", port) catch return 3);
    const served = net.accept(l) catch return 4;

    http.send_request(client, "127.0.0.1", "GET", "/", "") catch return 5;

    // Two chunks and the zero-length one that ends them, with an extension on
    // the first that a reader has to tolerate and ignore.
    net.write_all(served, "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5;x=1\r\nhello\r\n7\r\n, world\r\n0\r\n\r\n") catch return 6;

    const answer = http.read_response(client) catch return 7;
    print(answer.body);
    print_int(answer.code - 188);

    net.close(client.socket);
    net.close(served);
    net.close_listener(l);
    return 0;
}
