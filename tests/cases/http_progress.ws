// expect: progress ok
const net = @import("std/net");
const http = @import("std/http");
const text = @import("std/str");

const Progress = struct { bytes: i64, total: i64, calls: i64 };
fn check(response: str, total: i64) i64 {
    const listener = net.listen("127.0.0.1", 0, 4) catch return 1;
    const port = net.local_port(listener) catch return 2;
    const c = http.connection(net.connect("127.0.0.1", port) catch return 3);
    const server = net.accept(listener) catch return 4;
    net.write_all(server, response) catch return 5;
    const progress = Progress{ .bytes = 0, .total = -2, .calls = 0 };
    const result = http.read_response_progress(c, fn(received: i64, expected: i64) void {
        assert(received >= progress.bytes);
        progress.bytes = received;
        progress.total = expected;
        progress.calls += 1;
        return;
    }) catch return 6;
    assert(result.body == "hello world");
    assert(progress.bytes == 11);
    assert(progress.total == total);
    assert(progress.calls > 0);
    http.close(c);
    net.close(server);
    net.close_listener(listener);
    return 0;
}

fn main() i64 {
    assert(check("HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\nhello world", 11) == 0);
    assert(check("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n", -1) == 0);
    print("progress ok");
    return 0;
}
