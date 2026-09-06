// A TCP conversation held in buffers rather than in strings.
//
// Single-threaded and on port 0, for the reasons `net_tcp.ws` gives: `connect`
// completes into the backlog without anyone having accepted, so one thread can
// be both ends, and a hardcoded port is a test that fails whenever the machine
// happens to be using it.
//
// What this is here to show is that the byte API reuses one buffer for the
// whole connection. The `str` API above it allocates a fresh string per read,
// which is right for a protocol made of lines and wrong for one made of
// 16 KiB records.
// expect: bound
// expect: 00112233
// expect: 4
// expect: deadbeef
// expect: 0
// expect: reused
const net = @import("std/net");
const bytes = @import("std/bytes");
const array = @import("std/array");

fn main() i64 {
    const l = net.listen("127.0.0.1", 0, 4) catch return 1;
    const port = net.local_port(l) catch return 2;
    print("bound");

    const client = net.connect("127.0.0.1", port) catch return 3;
    const served = net.accept(l) catch return 4;

    // One buffer, used for every read and write below.
    const buf = bytes.new(64);

    const sent = bytes.from_hex("00112233") catch return 5;
    net.write_all_bytes(client, sent, 0, 4) catch return 6;
    net.read_exactly_into(served, buf, 0, 4) catch return 7;
    print(bytes.to_hex(bytes.slice(buf, 0, 4)));

    // Writing out of the middle of a buffer, and reading into the middle of
    // one -- which is what a record layer does with a header and a body.
    bytes.copy(buf, 8, bytes.from_hex("deadbeef") catch return 8, 0, 4);
    net.write_all_bytes(served, buf, 8, 4) catch return 9;
    const n = net.read_into(client, buf, 16, 4) catch return 10;
    print_int(n);
    print(bytes.to_hex(bytes.slice(buf, 16, 20)));

    // The far end finishing is a read of zero, not an error.
    net.close(served);
    print_int(net.read_into(client, buf, 32, 4) catch return 11);

    // The buffer is the same object it was: nothing here allocated one.
    if (array.len(buf) == 64) { print("reused"); }

    net.close(client);
    net.close_listener(l);
    return 0;
}
