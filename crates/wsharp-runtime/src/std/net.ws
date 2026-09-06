// Sockets: TCP over IPv4 or IPv6.
//
// The address family is the resolver's choice, not the program's. `connect`
// tries every address a name gives until one answers, which is what makes a
// dual-stack host work without the caller knowing there was a choice.
//
// The handles are numbers, because a socket belongs to the process rather than
// to any one worker's heap -- the same reason a broker topic is one. What makes
// them typed is here: a `Listener` accepts and a `Socket` reads and writes, so
// a program cannot read from the thing it should have been accepting on.
//
// Reading and writing block, and that is safe rather than merely tolerated: a
// builtin that blocks does so inside a safe region, so this worker's collector
// can walk its stack and run its pauses while the thread waits on the network.
// `set_nonblocking` is for a program that would rather be told
// `error.WouldBlock` than wait.
//
// The loops below are in W# rather than Rust for the usual reason: each moves
// references from one object into another, and generated code goes through the
// write barrier, the load barrier and the stack maps by construction.
const text = @import("std/str");
const list = @import("std/list");

/// One end of a connection.
pub const Socket = struct { handle: i64 };

/// A socket that only accepts.
pub const Listener = struct { handle: i64 };

/// Connect to `host:port`.
pub fn connect(host: str, port: i64) !Socket {
    return Socket{ .handle = try raw_connect(host, port) };
}

/// Listen on `host:port`, letting `backlog` connections queue.
///
/// An empty host means every interface. Port 0 means "any free port", and
/// `local_port` reports which one was granted -- which is what lets a test bind
/// a port without hoping that a chosen one is free.
pub fn listen(host: str, port: i64, backlog: i64) !Listener {
    return Listener{ .handle = try raw_listen(host, port, backlog) };
}

/// The port a listener is actually bound to.
pub fn local_port(l: Listener) !i64 {
    return try raw_local_port(l.handle);
}

/// Wait for the next connection.
pub fn accept(l: Listener) !Socket {
    return Socket{ .handle = try raw_accept(l.handle) };
}

/// At most `max` bytes. An empty result means the far end has finished.
pub fn read(s: Socket, max: i64) !str {
    return try raw_read(s.handle, max);
}

/// Write what the socket will take now, and report how much that was.
pub fn write(s: Socket, bytes: str) !i64 {
    return try raw_write(s.handle, bytes);
}

/// Write all of `bytes`, however many calls that takes.
///
/// A stream takes what it has room for and no more, so a `write` that returns a
/// smaller number than it was given is ordinary rather than a failure. A write
/// that takes nothing at all is not: nothing is draining the other end.
pub fn write_all(s: Socket, bytes: str) !void {
    const total = text.len(bytes);
    var sent = 0;
    while (sent < total) {
        const n = try raw_write(s.handle, text.substr(bytes, sent, total));
        if (n <= 0) { return error.BrokenPipe; }
        sent = sent + n;
    }
    return;
}

/// Exactly `n` bytes, or `error.EndOfFile` if the stream ends first.
pub fn read_exactly(s: Socket, n: i64) !str {
    var out = "";
    while (text.len(out) < n) {
        const chunk = try raw_read(s.handle, n - text.len(out));
        if (text.len(chunk) == 0) { return error.EndOfFile; }
        out = text.concat(out, chunk);
    }
    return out;
}

/// Everything until the far end finishes.
///
/// Only for a peer that closes when it is done. A protocol that keeps the
/// connection open between messages must say how long each one is, and use
/// `read_exactly`.
pub fn read_all(s: Socket) !str {
    var out = "";
    var reading = true;
    while (reading) {
        const chunk = try raw_read(s.handle, 65536);
        if (text.len(chunk) == 0) {
            reading = false;
        } else {
            out = text.concat(out, chunk);
        }
    }
    return out;
}

/// Ask to be told `error.WouldBlock` rather than to wait.
pub fn set_nonblocking(s: Socket, on: bool) !void {
    const done = try raw_set_nonblocking(s.handle, on);
    return;
}

/// The same, for a listener: `accept` then answers immediately either way.
pub fn accept_nonblocking(l: Listener, on: bool) !void {
    const done = try raw_set_nonblocking(l.handle, on);
    return;
}

/// Close a connection. Closing one twice is harmless.
pub fn close(s: Socket) void {
    raw_close(s.handle);
}

/// Stop listening, releasing the port.
pub fn close_listener(l: Listener) void {
    raw_close(l.handle);
}

// ---------------------------------------------------------------------------
// Datagrams
// ---------------------------------------------------------------------------
//
// A separate type from `Socket` rather than a flag on it, because almost
// nothing they can do is the same: there is no connection to read to the end
// of, every message carries its own address, and a message too big for the
// buffer is lost rather than waiting.

/// A bound datagram socket.
pub const Datagrams = struct { handle: i64 };

/// Somewhere a datagram came from.
///
/// A handle rather than a host and port, so that replying needs no address
/// formatting and no parsing back: the address is kept as the bytes the
/// operating system gave us.
pub const Peer = struct { handle: i64 };

/// One message, and who sent it.
pub const Datagram = struct { data: str, peer: Peer };

/// Bind a datagram socket. An empty host is every interface; port 0 is any free
/// port, which `udp_port` then reports.
pub fn udp(host: str, port: i64) !Datagrams {
    return Datagrams{ .handle = try raw_udp(host, port) };
}

/// The port a datagram socket is bound to.
pub fn udp_port(d: Datagrams) !i64 {
    return try raw_local_port(d.handle);
}

/// Send one message to a named address.
pub fn send_to(d: Datagrams, host: str, port: i64, bytes: str) !i64 {
    return try raw_send_to(d.handle, host, port, bytes);
}

/// Send one message back to where one came from.
pub fn reply(d: Datagrams, peer: Peer, bytes: str) !i64 {
    return try raw_send_peer(d.handle, peer.handle, bytes);
}

/// Receive one message, and who sent it.
///
/// A message longer than `max` is truncated and the rest is lost, which is what
/// a datagram is: the caller chooses the largest it is prepared to accept. The
/// sender is read back in a second call for the reason the poller's results
/// are -- one builtin answers with one value, and building the pair is W#'s
/// job.
pub fn receive(d: Datagrams, max: i64) !Datagram {
    const data = try raw_recv_from(d.handle, max);
    const peer = try raw_last_peer(d.handle);
    return Datagram{ .data = data, .peer = Peer{ .handle = peer } };
}

/// Ask to be told `error.WouldBlock` rather than to wait.
pub fn udp_nonblocking(d: Datagrams, on: bool) !void {
    const done = try raw_set_nonblocking(d.handle, on);
    return;
}

/// Watch a datagram socket for arriving messages.
pub fn watch_datagrams(p: Poller, d: Datagrams) !void {
    const done = try raw_watch(p.handle, d.handle, true, false);
    return;
}

pub fn close_datagrams(d: Datagrams) void {
    raw_close(d.handle);
}

// ---------------------------------------------------------------------------
// Readiness
// ---------------------------------------------------------------------------
//
// Blocking is safe here -- a builtin that blocks does so inside a safe region --
// so this is not what keeps the collector alive. It is what lets *one* worker
// serve many connections: a server that blocks in `read` serves one client at a
// time however many it has accepted.

/// A set of sockets to wait on.
pub const Poller = struct { handle: i64 };

/// What one wait found.
///
/// A socket whose far end has gone reads as `readable`, and the `read` that
/// follows is where that has a name -- so a loop that reads whatever it is told
/// is ready needs no second case for a closed connection.
pub const Event = struct { socket: Socket, readable: bool, writable: bool };

/// A poller watching nothing.
pub fn poller() !Poller {
    return Poller{ .handle = try raw_poller() };
}

/// Watch a connection, or change what it is watched for.
pub fn watch(p: Poller, s: Socket, readable: bool, writable: bool) !void {
    const done = try raw_watch(p.handle, s.handle, readable, writable);
    return;
}

/// Watch a listener. A listener with a connection waiting reads as readable,
/// which is when `accept` will not block.
pub fn watch_listener(p: Poller, l: Listener) !void {
    const done = try raw_watch(p.handle, l.handle, true, false);
    return;
}

/// Stop watching. Forgetting one that was never watched is not an error.
pub fn forget(p: Poller, s: Socket) !void {
    const done = try raw_forget(p.handle, s.handle);
    return;
}

/// Wait until something is ready, or `timeout_ms` passes.
///
/// A negative timeout waits indefinitely and zero polls. The results are read
/// back one at a time and assembled here, because a builtin may not build a
/// list: doing it in W# is what puts every reference through the write barrier
/// and into a stack map.
pub fn wait(p: Poller, timeout_ms: i64) !list.List[Event] {
    const found = try raw_wait(p.handle, timeout_ms);
    var out = list.new();
    var i = 0;
    while (i < found) : (i += 1) {
        const handle = try raw_ready_socket(p.handle, i);
        // 1 is readable and 2 is writable. Tested with arithmetic because W#
        // has no bitwise operators yet.
        const bits = try raw_ready_events(p.handle, i);
        list.push(out, Event{
            .socket = Socket{ .handle = handle },
            .readable = bits % 2 == 1,
            .writable = bits >= 2,
        });
    }
    return out;
}

/// Close a poller. The sockets it was watching are untouched.
pub fn close_poller(p: Poller) void {
    raw_close_poller(p.handle);
}
