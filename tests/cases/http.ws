// A whole HTTP exchange with itself, both ends in one process.
//
// Single-threaded, like the socket cases: `connect` completes into the backlog
// and every message here is small enough to fit in a socket buffer, so neither
// side ever waits for the other to be scheduled.
// expect: GET /hello
// expect: hostname 127.0.0.1
// expect: 200
// expect: not found
// expect: text/plain
// expect: hello, world
const net = @import("std/net");
const http = @import("std/http");
const str = @import("std/str");

/// Dispatch on the status *type*, which is what the lattice is for: this is
/// chosen by `Response.status`, not by comparing numbers.
fn describe(s: http.Status) str { return "other"; }
fn describe(s: http.Status2xx) str { return "ok"; }
fn describe(s: http.NotFound404) str { return "not found"; }

fn main() i64 {
    const l = net.listen("127.0.0.1", 0, 4) catch return 1;
    const port = net.local_port(l) catch return 2;

    const client = http.connection(net.connect("127.0.0.1", port) catch return 3);
    const served = http.connection(net.accept(l) catch return 4);

    http.send_request(client, "127.0.0.1", "GET", "/hello", "") catch return 5;

    const request = http.read_request(served) catch return 6;
    print(str.concat(str.concat(request.method, " "), request.path));
    print(str.concat("hostname ", http.header(request.headers, "HOST") orelse "?"));

    http.respond(served, 200, "text/plain", "hello, world") catch return 7;

    const answer = http.read_response(client) catch return 8;
    print_int(answer.code);
    // 404 is not what came back; this shows the type is a real one either way.
    print(describe(http.status_of(404)));
    print(http.header(answer.headers, "content-type") orelse "?");
    print(answer.body);

    net.close(client.socket);
    net.close(served.socket);
    net.close_listener(l);
    return 0;
}
