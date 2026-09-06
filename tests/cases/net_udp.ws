// Datagrams, and replying to one without ever writing down an address.
//
// A `Peer` is a handle to the address the message arrived from, so the server
// half needs no host, no port and no parsing -- which is also what keeps IPv6
// from costing anything here.
// expect: ping
// expect: pong
// expect: nothing yet
const net = @import("std/net");

fn main() i64 {
    const server = net.udp("127.0.0.1", 0) catch return 1;
    const client = net.udp("127.0.0.1", 0) catch return 2;
    const port = net.udp_port(server) catch return 3;

    const sent = net.send_to(client, "127.0.0.1", port, "ping") catch return 4;
    if (sent != 4) { return 5; }

    const asked = net.receive(server, 1024) catch return 6;
    print(asked.data);

    const answered = net.reply(server, asked.peer, "pong") catch return 7;
    if (answered != 4) { return 8; }

    const back = net.receive(client, 1024) catch return 9;
    print(back.data);

    // Nothing else is coming, and a non-blocking socket says so at once
    // instead of waiting for a message that will never arrive.
    net.udp_nonblocking(client, true) catch return 10;
    net.receive(client, 1024) catch |e| {
        if (e == error.WouldBlock) { print("nothing yet"); } else { print("wrong error"); }
        net.close_datagrams(client);
        net.close_datagrams(server);
        return 0;
    };
    return 11;
}
