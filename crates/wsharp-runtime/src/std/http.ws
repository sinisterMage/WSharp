// HTTP/1.1, over `std/net`.
//
// The 27 status types this module also holds are not declared here: they are a
// table in the compiler, materialised on first mention, so that a program pays
// only for the ones it names. `status_of` is the bridge -- it turns the number
// on the wire into the type, and from there `fn handle(r: NotFound404)` is an
// ordinary dispatch on an ordinary lattice.
const net = @import("std/net");
const text = @import("std/str");
const list = @import("std/list");
const tls = @import("std/tls");
const x509 = @import("std/x509");
const bytes = @import("std/bytes");
const array = @import("std/array");

/// One header. Names are compared lowercased, because the wire does not agree
/// about their case and nothing should have to care.
pub const Header = struct { name: str, value: str };

/// The type for a numeric status, or `Status` itself when the number is not one
/// this library knows.
///
/// Written as a chain rather than a table because a table would need an array
/// of a supertype and this needs none: every arm returns a subtype, and
/// widening a subtype into its supertype is free.
pub fn status_of(code: i64) Status {
    if (code == 100) { return Continue100; }
    if (code == 200) { return Ok200; }
    if (code == 201) { return Created201; }
    if (code == 202) { return Accepted202; }
    if (code == 204) { return NoContent204; }
    if (code == 301) { return MovedPermanently301; }
    if (code == 302) { return Found302; }
    if (code == 304) { return NotModified304; }
    if (code == 400) { return BadRequest400; }
    if (code == 401) { return Unauthorized401; }
    if (code == 403) { return Forbidden403; }
    if (code == 404) { return NotFound404; }
    if (code == 405) { return MethodNotAllowed405; }
    if (code == 409) { return Conflict409; }
    if (code == 418) { return Teapot418; }
    if (code == 429) { return TooManyRequests429; }
    if (code == 500) { return ServerError500; }
    if (code == 501) { return NotImplemented501; }
    if (code == 502) { return BadGateway502; }
    if (code == 503) { return ServiceUnavailable503; }
    if (code == 504) { return GatewayTimeout504; }
    // An unknown number still belongs to its class, which is what a caller
    // dispatching on `Status4xx` actually wants to know.
    if (code >= 100 and code < 200) { return Status1xx; }
    if (code >= 200 and code < 300) { return Status2xx; }
    if (code >= 300 and code < 400) { return Status3xx; }
    if (code >= 400 and code < 500) { return Status4xx; }
    if (code >= 500 and code < 600) { return Status5xx; }
    return Status;
}

/// The reason phrase for a code. Only what the lattice knows; anything else is
/// a bare word, because a made-up phrase is worse than a plain one.
pub fn reason(code: i64) str {
    if (code == 200) { return "OK"; }
    if (code == 201) { return "Created"; }
    if (code == 204) { return "No Content"; }
    if (code == 301) { return "Moved Permanently"; }
    if (code == 302) { return "Found"; }
    if (code == 304) { return "Not Modified"; }
    if (code == 400) { return "Bad Request"; }
    if (code == 401) { return "Unauthorized"; }
    if (code == 403) { return "Forbidden"; }
    if (code == 404) { return "Not Found"; }
    if (code == 405) { return "Method Not Allowed"; }
    if (code == 409) { return "Conflict"; }
    if (code == 418) { return "I'm a teapot"; }
    if (code == 429) { return "Too Many Requests"; }
    if (code == 500) { return "Internal Server Error"; }
    if (code == 501) { return "Not Implemented"; }
    if (code == 502) { return "Bad Gateway"; }
    if (code == 503) { return "Service Unavailable"; }
    if (code == 504) { return "Gateway Timeout"; }
    return "Status";
}

/// A request, as a client sends it or a server reads it.
pub const Request = struct {
    method: str,
    path: str,
    headers: list.List[Header],
    body: str,
};

/// A response, as a server sends it or a client reads it.
///
/// `code` is the number on the wire and `status` is the type it belongs to, so
/// that `fn handle(s: NotFound404)` and `if (r.code == 404)` are both available
/// and neither has to be derived from the other at every use.
pub const Response = struct {
    code: i64,
    status: Status,
    headers: list.List[Header],
    body: str,
};

/// A socket and what has been read from it but not yet used.
///
/// A protocol that reads lines needs somewhere to put the rest of the packet
/// the line arrived in -- a socket cannot be read back into. This is that
/// somewhere, and it is why every read below goes through a `Conn` rather than
/// through the socket directly.
pub const Conn = struct {
    socket: net.Socket,
    buffered: str,
    /// The TLS session in front of the socket, or null for `http://`.
    session: ?tls.Session,
};

pub fn connection(s: net.Socket) Conn {
    return Conn{ .socket = s, .buffered = "", .session = null };
}

pub fn tls_connection(s: tls.Session) Conn {
    return Conn{ .socket = s.socket, .buffered = "", .session = s };
}

// A field and an `if`, where a subtype and an overload set would read better.
//
// The lattice is what this language is for and it was the first thing tried
// here: `TlsConn : Conn`, with `conn_read` and `conn_write` as overloads, so
// that the four lines below that touch a socket became two functions and
// nothing else in the module changed. It does not work, and the reason is
// worth writing down. **A dispatched call has one type, so every overload must
// share it** -- and these two do not: reading through TLS can raise everything
// a handshake can, which is two dozen names, where reading a socket raises ten.
// Making them agree means writing that whole set out twice, on both overloads,
// and keeping the two copies in step for ever; or catching inside and
// answering with one flattened error, which throws away the reason a
// connection failed. Neither is worth the shape.

/// Read from whatever kind of connection this is.
fn conn_read(c: Conn, max: i64) !str {
    if (c.session) |s| {
        const buf = bytes.new(max);
        const n = try tls.read(s, buf, 0, max);
        return bytes.slice_str(buf, 0, n);
    }
    const chunk = try net.read(c.socket, max);
    return chunk;
}

fn conn_write(c: Conn, data: str) !void {
    if (c.session) |s| {
        const b = bytes.of(data);
        try tls.write_all(s, b, 0, array.len(b));
        return;
    }
    try net.write_all(c.socket, data);
    return;
}

/// Close a connection, telling the peer first where there is a way to.
pub fn close(c: Conn) void {
    if (c.session) |s| {
        // A close_notify is what distinguishes a finished response from a
        // truncated one, so it goes even though the socket is about to.
        tls.close_session(s);
        return;
    }
    net.close(c.socket);
    return;
}

/// The value of a header, matched without regard to case; null when absent.
pub fn header(headers: list.List[Header], name: str) ?str {
    const wanted = text.to_lower(name);
    var i = 0;
    while (i < list.len(headers)) : (i += 1) {
        const h = list.get(headers, i);
        if (h.name == wanted) { return h.value; }
    }
    return null;
}

/// One line, without its terminator.
fn read_line(c: Conn) !str {
    var at = text.find(c.buffered, "\r\n");
    while (at < 0) {
        const chunk = try conn_read(c, 4096);
        if (text.len(chunk) == 0) { return error.EndOfFile; }
        c.buffered = text.concat(c.buffered, chunk);
        at = text.find(c.buffered, "\r\n");
    }
    const line = text.substr(c.buffered, 0, at);
    c.buffered = text.substr(c.buffered, at + 2, text.len(c.buffered));
    return line;
}

/// Exactly `n` bytes of body.
fn read_body(c: Conn, n: i64) !str {
    while (text.len(c.buffered) < n) {
        const chunk = try conn_read(c, n - text.len(c.buffered));
        if (text.len(chunk) == 0) { return error.EndOfFile; }
        c.buffered = text.concat(c.buffered, chunk);
    }
    const body = text.substr(c.buffered, 0, n);
    c.buffered = text.substr(c.buffered, n, text.len(c.buffered));
    return body;
}

/// Header lines, up to the blank one that ends them.
fn read_headers(c: Conn) !list.List[Header] {
    var out = list.new();
    var reading = true;
    while (reading) {
        const line = try read_line(c);
        if (text.len(line) == 0) {
            reading = false;
        } else {
            const colon = text.find(line, ":");
            if (colon < 0) { return error.BadFormat; }
            list.push(out, Header{
                .name = text.to_lower(text.trim(text.substr(line, 0, colon))),
                .value = text.trim(text.substr(line, colon + 1, text.len(line))),
            });
        }
    }
    return out;
}

/// A hexadecimal number, which is how a chunked body says how long each piece
/// is. Written here because `str.parse_int` is decimal and a chunk header is
/// the only place W# meets base 16.
fn parse_hex(s: str) !i64 {
    if (text.len(s) == 0) { return error.BadFormat; }
    var value = 0;
    var i = 0;
    while (i < text.len(s)) : (i += 1) {
        const b = text.byte_at(s, i);
        var digit = -1;
        if (b >= 48 and b <= 57) { digit = b - 48; }
        if (b >= 97 and b <= 102) { digit = b - 87; }
        if (b >= 65 and b <= 70) { digit = b - 55; }
        if (digit < 0) { return error.BadFormat; }
        value = (value << 4) | digit;
    }
    return value;
}

/// A body sent in pieces, each announced by its length.
fn read_chunked(c: Conn) !str {
    var body = "";
    var more = true;
    while (more) {
        const announced = try read_line(c);
        // A chunk size may carry extensions after a `;`, which nothing here
        // uses and everything here must tolerate.
        const semi = text.find(announced, ";");
        var digits = announced;
        if (semi >= 0) { digits = text.substr(announced, 0, semi); }
        const size = try parse_hex(text.trim(digits));
        if (size == 0) {
            more = false;
        } else {
            body = text.concat(body, try read_body(c, size));
            // The CRLF that follows every chunk's bytes.
            const after = try read_line(c);
        }
    }
    // Trailers, if any, then the blank line that ends them. With no trailers
    // the first line read is already blank.
    var trailing = true;
    while (trailing) {
        const line = try read_line(c);
        if (text.len(line) == 0) { trailing = false; }
    }
    return body;
}

/// The body a set of headers describes.
fn read_payload(c: Conn, headers: list.List[Header]) !str {
    const encoding = header(headers, "Transfer-Encoding") orelse "";
    if (text.to_lower(encoding) == "chunked") { return try read_chunked(c); }
    const declared = header(headers, "Content-Length") orelse "";
    if (text.len(declared) == 0) { return ""; }
    return try read_body(c, try text.parse_int(declared));
}

/// Where a URL points.
const Endpoint = struct { host: str, port: i64, path: str, secure: bool };

/// `http://host[:port][/path]` or `https://` the same.
fn parse_url(url: str) !Endpoint {
    var secure = false;
    var skip = 7;
    if (text.starts_with(url, "https://")) {
        secure = true;
        skip = 8;
    } else {
        if (!text.starts_with(url, "http://")) { return error.BadFormat; }
    }

    const rest = text.substr(url, skip, text.len(url));
    var authority = rest;
    var path = "/";
    const slash = text.find(rest, "/");
    if (slash >= 0) {
        authority = text.substr(rest, 0, slash);
        path = text.substr(rest, slash, text.len(rest));
    }
    if (text.len(authority) == 0) { return error.BadFormat; }

    var host = authority;
    var port = 80;
    if (secure) { port = 443; }
    const colon = text.find(authority, ":");
    if (colon >= 0) {
        host = text.substr(authority, 0, colon);
        port = try text.parse_int(text.substr(authority, colon + 1, text.len(authority)));
    }
    return Endpoint{ .host = host, .port = port, .path = path, .secure = secure };
}

/// Write a request line, headers and body onto a connection.
///
/// Separate from `request` so that a program which already has a connection --
/// a test with both ends in one process, or a client reusing a socket -- can
/// use it without opening another.
pub fn send_request(c: Conn, host: str, method: str, path: str, body: str) !void {
    var head = text.concat(method, " ");
    head = text.concat(head, path);
    head = text.concat(head, " HTTP/1.1\r\nHost: ");
    head = text.concat(head, host);
    head = text.concat(head, "\r\nConnection: close\r\nContent-Length: ");
    head = text.concat(head, text.from_int(text.len(body)));
    head = text.concat(head, "\r\n\r\n");
    try conn_write(c, text.concat(head, body));
    return;
}

/// Read a response off a connection, once a request has been sent.
pub fn read_response(c: Conn) !Response {
    // "HTTP/1.1 200 OK"
    const line = try read_line(c);
    const after_version = text.find(line, " ");
    if (after_version < 0) { return error.BadFormat; }
    const rest = text.substr(line, after_version + 1, text.len(line));
    var digits = rest;
    const after_code = text.find(rest, " ");
    if (after_code >= 0) { digits = text.substr(rest, 0, after_code); }
    const code = try text.parse_int(digits);

    const headers = try read_headers(c);
    const body = try read_payload(c, headers);
    return Response{
        .code = code,
        .status = status_of(code),
        .headers = headers,
        .body = body,
    };
}

/// Send one request and read one response.
///
/// `Connection: close` on purpose: this opens a socket per request and closes
/// it, which is the honest shape for a client with no pool. A protocol that
/// wants to reuse a connection can build one out of `Conn`, `read_response`
/// and the writer below.
pub fn request(url: str, method: str, body: str) !Response {
    const where = try parse_url(url);
    if (where.secure) {
        // Reading and parsing the store costs about as much as the handshake
        // does, and doing it per request is why `request_with` exists.
        const roots = try x509.system_roots();
        return try request_with(url, method, body,
                                tls.roots_config(where.host, roots));
    }
    return try request_with(url, method, body, tls.client_config(""));
}

/// The same, against a TLS configuration the caller already has.
///
/// A program making more than one `https://` request wants this one: a
/// `Config` holds the parsed trust store, and parsing it is the expensive half
/// of a connection. The configuration's host is ignored -- the URL says which
/// host this is, and a certificate checked against the wrong name is worse
/// than none.
pub fn request_with(url: str, method: str, body: str, cfg: tls.Config) !Response {
    const where = try parse_url(url);
    const socket = try net.connect(where.host, where.port);
    var c = connection(socket);
    if (where.secure) {
        const session = try tls.connect(socket, with_host(cfg, where.host));
        c = tls_connection(session);
    }
    try send_request(c, where.host, method, where.path, body);
    const answer = try read_response(c);
    close(c);
    return answer;
}

/// The same configuration, aimed at this host.
fn with_host(cfg: tls.Config, host: str) tls.Config {
    return tls.Config{ .host = host, .pinned = cfg.pinned, .roots = cfg.roots };
}

pub fn get(url: str) !Response {
    return try request(url, "GET", "");
}

pub fn post(url: str, body: str) !Response {
    return try request(url, "POST", body);
}

// ---------------------------------------------------------------------------
// The server's half
// ---------------------------------------------------------------------------

/// Read one request. `error.EndOfFile` when the peer has gone.
pub fn read_request(c: Conn) !Request {
    // "GET /path HTTP/1.1"
    const line = try read_line(c);
    const after_method = text.find(line, " ");
    if (after_method < 0) { return error.BadFormat; }
    const method = text.substr(line, 0, after_method);
    const rest = text.substr(line, after_method + 1, text.len(line));
    var path = rest;
    const after_path = text.find(rest, " ");
    if (after_path >= 0) { path = text.substr(rest, 0, after_path); }

    const headers = try read_headers(c);
    const body = try read_payload(c, headers);
    return Request{
        .method = method,
        .path = path,
        .headers = headers,
        .body = body,
    };
}

/// Write a response and say the connection is over.
///
/// The length is always declared, so a client need never guess and never has
/// to read to end of stream to know it has everything.
pub fn respond(c: Conn, code: i64, content_type: str, body: str) !void {
    var head = "HTTP/1.1 ";
    head = text.concat(head, text.from_int(code));
    head = text.concat(head, " ");
    head = text.concat(head, reason(code));
    head = text.concat(head, "\r\nContent-Type: ");
    head = text.concat(head, content_type);
    head = text.concat(head, "\r\nContent-Length: ");
    head = text.concat(head, text.from_int(text.len(body)));
    head = text.concat(head, "\r\nConnection: close\r\n\r\n");
    try conn_write(c, text.concat(head, body));
    return;
}
