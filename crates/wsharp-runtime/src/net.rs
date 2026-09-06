//! Sockets, as the builtins `std/net` is written over.
//!
//! Three rules shape everything here, and all three are the collector's.
//!
//! **A handle is a number, not an object.** A socket belongs to the process
//! rather than to any one worker's heap, exactly as a broker topic does, so it
//! is an index into a table here and W# holds the index in a one-field struct.
//! Nothing crosses as a pointer that a collector would then have to know about.
//!
//! **A builtin may read and write bytes; anything that moves a reference is
//! written in W#.** So the surface below deals in `str` -- which in W# is
//! arbitrary bytes, not text -- and never in arrays or structs. Anything that
//! builds a `[]T` or a `List[T]` lives in `std/net.ws`, where the write
//! barrier, the load barrier and the stack maps come for free.
//!
//! **Every blocking call is made inside a safe region.** `worker::blocking`
//! hands the collector a frame pointer and lets it run this worker's pauses
//! while the thread waits on the network -- which is the whole reason the file
//! half of `sys` was rewritten before the socket half was written at all.
//! Arguments are copied out of the heap before the region is entered, for the
//! same reason an allocating builtin copies before it allocates.

use std::sync::Mutex;

use crate::builtins::ERROR_IO_FAILED;
use crate::io::FallibleI64;
use crate::strings::{alloc_str, str_bytes};
use crate::sys::{self, Fd};

/// What a handle names.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A connected stream, or one accepted from a listener.
    Stream,
    /// A socket that only ever accepts.
    Listener,
    /// A datagram socket: no connection, and every message carries its own
    /// address.
    Datagram,
}

#[derive(Clone, Copy)]
struct Entry {
    fd: Fd,
    kind: Kind,
    /// Who sent the last datagram received on this socket, as a peer handle, or
    /// -1. Recorded rather than returned because one builtin answers with one
    /// value: `recv_from` hands back the bytes and this is read after, the same
    /// split the poller's `wait` and `ready_socket` use, and for the same
    /// reason -- a builtin may not build the struct that would hold both.
    last_peer: i64,
}

/// Open sockets, indexed by the handle W# holds.
///
/// A slot is emptied rather than removed when a socket is closed, so a handle
/// is never reused while a program might still be holding it: closing twice, or
/// reading from a closed socket, is then a reported error rather than an
/// operation on whatever opened next.
static SOCKETS: Mutex<Vec<Option<Entry>>> = Mutex::new(Vec::new());

fn with_sockets<R>(f: impl FnOnce(&mut Vec<Option<Entry>>) -> R) -> R {
    let mut guard = SOCKETS.lock().unwrap_or_else(|e| e.into_inner());
    f(&mut guard)
}

fn insert(fd: Fd, kind: Kind) -> i64 {
    with_sockets(|table| {
        table.push(Some(Entry {
            fd,
            kind,
            last_peer: -1,
        }));
        (table.len() - 1) as i64
    })
}

/// The socket a handle names, whatever kind it is. For the operations that do
/// not care -- asking a port, setting non-blocking, closing.
fn lookup_any(handle: i64) -> Option<Fd> {
    if handle < 0 {
        return None;
    }
    with_sockets(|table| Some((*table.get(handle as usize)?)?.fd))
}

/// Addresses datagrams arrived from, indexed by the peer handle W# holds.
///
/// Append-only, and never large: a peer is remembered so that a reply can be
/// sent to it, and a server that talks to many clients accumulates one entry
/// per datagram it answers. Addresses are bytes and belong to no heap, which is
/// what lets them live here rather than in one.
static PEERS: Mutex<Vec<sys::SockAddr>> = Mutex::new(Vec::new());

fn remember_peer(addr: sys::SockAddr) -> i64 {
    let mut guard = PEERS.lock().unwrap_or_else(|e| e.into_inner());
    guard.push(addr);
    (guard.len() - 1) as i64
}

fn peer_at(handle: i64) -> Option<sys::SockAddr> {
    if handle < 0 {
        return None;
    }
    let guard = PEERS.lock().unwrap_or_else(|e| e.into_inner());
    guard.get(handle as usize).copied()
}

/// The socket a handle names, if it is one and is still open.
fn lookup(handle: i64, kind: Kind) -> Option<Fd> {
    if handle < 0 {
        return None;
    }
    with_sockets(|table| {
        let entry = (*table.get(handle as usize)?)?;
        (entry.kind == kind).then_some(entry.fd)
    })
}

/// A handle that names nothing. Not a panic: a program that closed a socket
/// twice, or kept a handle past `close`, gets an error it can catch.
fn no_such_socket() -> i64 {
    ERROR_IO_FAILED
}

/// The port a W# `i64` carries, if it is one.
fn port_of(port: i64) -> Option<u16> {
    u16::try_from(port).ok()
}

// ---------------------------------------------------------------------------
// The builtins
// ---------------------------------------------------------------------------

/// Connect to `host:port`, returning a handle.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; `host` must be null or
/// point at a W# string object, and `out` at storage laid out as a
/// [`FallibleI64`].
pub unsafe extern "C" fn ws_net_connect(out: *mut FallibleI64, host: *const u8, port: i64) {
    unsafe { crate::gc::checkpoint() };
    // Out of the heap before the safe region: nothing in there may touch it.
    let host = unsafe { str_bytes(host) }.to_vec();
    let Some(port) = port_of(port) else {
        unsafe { out.write(FallibleI64::err(crate::builtins::ERROR_HOST_NOT_FOUND)) };
        return;
    };
    let connected = crate::worker::blocking(|| {
        sys::resolve(&host, port, true, false).and_then(|addrs| sys::tcp_connect(&addrs))
    });
    let result = match connected {
        Ok(fd) => FallibleI64::ok(insert(fd, Kind::Stream)),
        Err(e) => FallibleI64::err(sys::error_tag(e)),
    };
    unsafe { out.write(result) };
}

/// Listen on `host:port`. An empty host means every interface, and port 0 means
/// "any free port", which `local_port` then reports.
///
/// # Safety
/// As [`ws_net_connect`].
pub unsafe extern "C" fn ws_net_listen(
    out: *mut FallibleI64,
    host: *const u8,
    port: i64,
    backlog: i64,
) {
    unsafe { crate::gc::checkpoint() };
    let host = unsafe { str_bytes(host) }.to_vec();
    let Some(port) = port_of(port) else {
        unsafe { out.write(FallibleI64::err(crate::builtins::ERROR_HOST_NOT_FOUND)) };
        return;
    };
    let backlog = backlog.clamp(1, 1024) as i32;
    let bound = crate::worker::blocking(|| {
        sys::resolve(&host, port, true, true).and_then(|addrs| sys::tcp_listen(&addrs, backlog))
    });
    let result = match bound {
        Ok(fd) => FallibleI64::ok(insert(fd, Kind::Listener)),
        Err(e) => FallibleI64::err(sys::error_tag(e)),
    };
    unsafe { out.write(result) };
}

/// Take the next connection waiting on a listener.
///
/// # Safety
/// As [`ws_net_connect`].
pub unsafe extern "C" fn ws_net_accept(out: *mut FallibleI64, listener: i64) {
    unsafe { crate::gc::checkpoint() };
    let Some(fd) = lookup(listener, Kind::Listener) else {
        unsafe { out.write(FallibleI64::err(no_such_socket())) };
        return;
    };
    let taken = crate::worker::blocking(|| sys::accept(fd));
    let result = match taken {
        Ok(fd) => FallibleI64::ok(insert(fd, Kind::Stream)),
        Err(e) => FallibleI64::err(sys::error_tag(e)),
    };
    unsafe { out.write(result) };
}

/// At most `max` bytes from a socket. An empty result is the end of the stream.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; `out` must point at
/// storage laid out as a [`crate::io::FallibleStr`].
pub unsafe extern "C" fn ws_net_read(out: *mut crate::io::FallibleStr, socket: i64, max: i64) {
    unsafe { crate::gc::checkpoint() };
    let Some(fd) = lookup(socket, Kind::Stream) else {
        unsafe { out.write(crate::io::FallibleStr::err(no_such_socket())) };
        return;
    };
    // A request for nothing is not an error, and must not be confused with the
    // empty read that means the far end has gone.
    let max = max.clamp(0, MAX_READ) as usize;
    let mut buf = vec![0u8; max];
    let read = crate::worker::blocking(|| sys::recv(fd, &mut buf));
    let result = match read {
        // The allocation happens outside the region, which is what lets it be
        // a safepoint like any other.
        Ok(n) => crate::io::FallibleStr::ok(alloc_str(&buf[..n])),
        Err(e) => crate::io::FallibleStr::err(sys::error_tag(e)),
    };
    unsafe { out.write(result) };
}

/// A single read is capped so that a program asking for a gigabyte does not get
/// a gigabyte of zeroed buffer before the first byte arrives. A caller that
/// wants more loops, which it has to anyway: a stream may return short.
const MAX_READ: i64 = 1 << 20;

/// Write bytes to a socket, returning how many were taken.
///
/// # Safety
/// As [`ws_net_connect`]; `bytes` must be null or a W# string object.
pub unsafe extern "C" fn ws_net_write(out: *mut FallibleI64, socket: i64, bytes: *const u8) {
    unsafe { crate::gc::checkpoint() };
    let Some(fd) = lookup(socket, Kind::Stream) else {
        unsafe { out.write(FallibleI64::err(no_such_socket())) };
        return;
    };
    let bytes = unsafe { str_bytes(bytes) }.to_vec();
    let sent = crate::worker::blocking(|| sys::send(fd, &bytes));
    let result = match sent {
        Ok(n) => FallibleI64::ok(n as i64),
        Err(e) => FallibleI64::err(sys::error_tag(e)),
    };
    unsafe { out.write(result) };
}

/// The port a listener is actually bound to.
///
/// # Safety
/// As [`ws_net_connect`].
pub unsafe extern "C" fn ws_net_local_port(out: *mut FallibleI64, listener: i64) {
    unsafe { crate::gc::checkpoint() };
    let Some(fd) = lookup_any(listener) else {
        unsafe { out.write(FallibleI64::err(no_such_socket())) };
        return;
    };
    let result = match sys::local_addr(fd) {
        Ok(addr) => FallibleI64::ok(i64::from(addr.port())),
        Err(e) => FallibleI64::err(sys::error_tag(e)),
    };
    unsafe { out.write(result) };
}

/// Put a socket into or out of non-blocking mode.
///
/// Answers `!i64` rather than `!void` and always with zero: the value is never
/// read, and a payload keeps this the same two-word shape as every other row
/// here, which is one fewer ABI to be right about.
///
/// # Safety
/// As [`ws_net_connect`].
pub unsafe extern "C" fn ws_net_set_nonblocking(out: *mut FallibleI64, socket: i64, on: i8) {
    unsafe { crate::gc::checkpoint() };
    let Some(fd) = lookup_any(socket) else {
        unsafe { out.write(FallibleI64::err(no_such_socket())) };
        return;
    };
    let result = match sys::set_nonblocking(fd, on != 0) {
        Ok(()) => FallibleI64::ok(0),
        Err(e) => FallibleI64::err(sys::error_tag(e)),
    };
    unsafe { out.write(result) };
}

/// Close a socket and forget its handle. Closing twice is harmless.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary.
pub unsafe extern "C" fn ws_net_close(socket: i64) {
    unsafe { crate::gc::checkpoint() };
    let taken = with_sockets(|table| {
        if socket < 0 {
            return None;
        }
        table.get_mut(socket as usize)?.take()
    });
    if let Some(entry) = taken {
        // Closing can block on a socket with data still queued.
        crate::worker::blocking(|| sys::close_socket(entry.fd));
    }
}

// ---------------------------------------------------------------------------
// Datagrams
// ---------------------------------------------------------------------------

/// Bind a datagram socket. An empty host is every interface and port 0 is any
/// free port, exactly as for a listener.
///
/// # Safety
/// As [`ws_net_connect`].
pub unsafe extern "C" fn ws_net_udp(out: *mut FallibleI64, host: *const u8, port: i64) {
    unsafe { crate::gc::checkpoint() };
    let host = unsafe { str_bytes(host) }.to_vec();
    let Some(port) = port_of(port) else {
        unsafe { out.write(FallibleI64::err(crate::builtins::ERROR_HOST_NOT_FOUND)) };
        return;
    };
    let bound = crate::worker::blocking(|| {
        sys::resolve(&host, port, false, true).and_then(|addrs| sys::udp_bind(&addrs))
    });
    let result = match bound {
        Ok(fd) => FallibleI64::ok(insert(fd, Kind::Datagram)),
        Err(e) => FallibleI64::err(sys::error_tag(e)),
    };
    unsafe { out.write(result) };
}

/// Send one datagram to a named address.
///
/// # Safety
/// As [`ws_net_connect`].
pub unsafe extern "C" fn ws_net_send_to(
    out: *mut FallibleI64,
    socket: i64,
    host: *const u8,
    port: i64,
    bytes: *const u8,
) {
    unsafe { crate::gc::checkpoint() };
    let Some(fd) = lookup(socket, Kind::Datagram) else {
        unsafe { out.write(FallibleI64::err(no_such_socket())) };
        return;
    };
    let host = unsafe { str_bytes(host) }.to_vec();
    let bytes = unsafe { str_bytes(bytes) }.to_vec();
    let Some(port) = port_of(port) else {
        unsafe { out.write(FallibleI64::err(crate::builtins::ERROR_HOST_NOT_FOUND)) };
        return;
    };
    let sent = crate::worker::blocking(|| {
        // Each address in turn, as a stream connection does: a name commonly
        // resolves to an IPv6 address this machine cannot reach and an IPv4 one
        // it can, and only trying tells them apart.
        let addrs = sys::resolve(&host, port, false, false)?;
        let mut last = None;
        for addr in &addrs {
            match sys::send_to(fd, addr, &bytes) {
                Ok(n) => return Ok(n),
                Err(e) => last = Some(e),
            }
        }
        Err(last.unwrap_or_else(sys::io_failed))
    });
    let result = match sent {
        Ok(n) => FallibleI64::ok(n as i64),
        Err(e) => FallibleI64::err(sys::error_tag(e)),
    };
    unsafe { out.write(result) };
}

/// Send one datagram back to where one came from.
///
/// # Safety
/// As [`ws_net_connect`].
pub unsafe extern "C" fn ws_net_send_peer(
    out: *mut FallibleI64,
    socket: i64,
    peer: i64,
    bytes: *const u8,
) {
    unsafe { crate::gc::checkpoint() };
    let fd = lookup(socket, Kind::Datagram);
    let addr = peer_at(peer);
    let (Some(fd), Some(addr)) = (fd, addr) else {
        unsafe { out.write(FallibleI64::err(no_such_socket())) };
        return;
    };
    let bytes = unsafe { str_bytes(bytes) }.to_vec();
    let sent = crate::worker::blocking(|| sys::send_to(fd, &addr, &bytes));
    let result = match sent {
        Ok(n) => FallibleI64::ok(n as i64),
        Err(e) => FallibleI64::err(sys::error_tag(e)),
    };
    unsafe { out.write(result) };
}

/// Receive one datagram, remembering where it came from.
///
/// A message longer than `max` is truncated and the rest is lost, which is what
/// UDP does: the caller chooses the largest message it will accept.
///
/// # Safety
/// As [`ws_net_read`].
pub unsafe extern "C" fn ws_net_recv_from(out: *mut crate::io::FallibleStr, socket: i64, max: i64) {
    unsafe { crate::gc::checkpoint() };
    let Some(fd) = lookup(socket, Kind::Datagram) else {
        unsafe { out.write(crate::io::FallibleStr::err(no_such_socket())) };
        return;
    };
    let max = max.clamp(0, MAX_READ) as usize;
    let mut buf = vec![0u8; max];
    let got = crate::worker::blocking(|| sys::recv_from(fd, &mut buf));
    let result = match got {
        Ok((n, from)) => {
            let peer = remember_peer(from);
            with_sockets(|table| {
                if let Some(Some(entry)) = table.get_mut(socket as usize) {
                    entry.last_peer = peer;
                }
            });
            crate::io::FallibleStr::ok(alloc_str(&buf[..n]))
        }
        Err(e) => crate::io::FallibleStr::err(sys::error_tag(e)),
    };
    unsafe { out.write(result) };
}

/// Who sent the datagram most recently received on this socket.
///
/// # Safety
/// As [`ws_net_connect`].
pub unsafe extern "C" fn ws_net_last_peer(out: *mut FallibleI64, socket: i64) {
    unsafe { crate::gc::checkpoint() };
    let peer = with_sockets(|table| {
        if socket < 0 {
            return None;
        }
        let entry = (*table.get(socket as usize)?)?;
        (entry.last_peer >= 0).then_some(entry.last_peer)
    });
    let result = match peer {
        Some(peer) => FallibleI64::ok(peer),
        None => FallibleI64::err(no_such_socket()),
    };
    unsafe { out.write(result) };
}

// ---------------------------------------------------------------------------
// Readiness
// ---------------------------------------------------------------------------

/// One poller: the set it is watching, and what the last wait found.
///
/// The results are kept here rather than returned because a builtin may not
/// build an array -- an array's type id is per instantiation and belongs to the
/// code generator, and a reference moved into one by hand would have been
/// through none of the three barriers. So `wait` reports how many are ready and
/// `ready_socket`/`ready_events` read them out one at a time, and `std/net.ws`
/// assembles the list where doing so is safe by construction.
struct Watch {
    poller: sys::Poller,
    /// Handle and descriptor for everything watched, so a result can be
    /// reported as the handle W# knows rather than the descriptor it does not.
    watched: Vec<(i64, Fd)>,
    /// What the last `wait` found: handle, readable, writable.
    last: Vec<(i64, bool, bool)>,
}

static POLLERS: Mutex<Vec<Option<Watch>>> = Mutex::new(Vec::new());

fn with_pollers<R>(f: impl FnOnce(&mut Vec<Option<Watch>>) -> R) -> R {
    let mut guard = POLLERS.lock().unwrap_or_else(|e| e.into_inner());
    f(&mut guard)
}

/// Run `f` on the poller a handle names.
fn with_watch<R>(handle: i64, f: impl FnOnce(&mut Watch) -> R) -> Option<R> {
    if handle < 0 {
        return None;
    }
    with_pollers(|table| {
        let slot = table.get_mut(handle as usize)?;
        slot.as_mut().map(f)
    })
}

/// A new poller, watching nothing.
///
/// # Safety
/// As [`ws_net_connect`].
pub unsafe extern "C" fn ws_net_poller(out: *mut FallibleI64) {
    unsafe { crate::gc::checkpoint() };
    let result = match sys::Poller::new() {
        Ok(poller) => {
            let handle = with_pollers(|table| {
                table.push(Some(Watch {
                    poller,
                    watched: Vec::new(),
                    last: Vec::new(),
                }));
                (table.len() - 1) as i64
            });
            FallibleI64::ok(handle)
        }
        Err(e) => FallibleI64::err(sys::error_tag(e)),
    };
    unsafe { out.write(result) };
}

/// Watch a socket, or change what it is watched for.
///
/// # Safety
/// As [`ws_net_connect`].
pub unsafe extern "C" fn ws_net_watch(
    out: *mut FallibleI64,
    poller: i64,
    socket: i64,
    readable: i8,
    writable: i8,
) {
    unsafe { crate::gc::checkpoint() };
    let Some(fd) = lookup_any(socket) else {
        unsafe { out.write(FallibleI64::err(no_such_socket())) };
        return;
    };
    let done = with_watch(poller, |watch| {
        watch.poller.watch(fd, readable != 0, writable != 0)?;
        if !watch.watched.iter().any(|(h, _)| *h == socket) {
            watch.watched.push((socket, fd));
        }
        Ok(())
    });
    let result = match done {
        None => FallibleI64::err(no_such_socket()),
        Some(Ok(())) => FallibleI64::ok(0),
        Some(Err(e)) => FallibleI64::err(sys::error_tag(e)),
    };
    unsafe { out.write(result) };
}

/// Stop watching a socket.
///
/// # Safety
/// As [`ws_net_connect`].
pub unsafe extern "C" fn ws_net_forget(out: *mut FallibleI64, poller: i64, socket: i64) {
    unsafe { crate::gc::checkpoint() };
    let done = with_watch(poller, |watch| {
        let fd = watch
            .watched
            .iter()
            .find(|(h, _)| *h == socket)
            .map(|(_, fd)| *fd);
        watch.watched.retain(|(h, _)| *h != socket);
        match fd {
            Some(fd) => watch.poller.forget(fd),
            // Forgetting one that was never watched is not a failure.
            None => Ok(()),
        }
    });
    let result = match done {
        None => FallibleI64::err(no_such_socket()),
        Some(Ok(())) => FallibleI64::ok(0),
        Some(Err(e)) => FallibleI64::err(sys::error_tag(e)),
    };
    unsafe { out.write(result) };
}

/// Wait for something to be ready, and report how many things are.
///
/// # Safety
/// As [`ws_net_connect`].
pub unsafe extern "C" fn ws_net_wait(out: *mut FallibleI64, poller: i64, timeout_ms: i64) {
    unsafe { crate::gc::checkpoint() };
    let timeout = timeout_ms.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    // The one call in the whole layer whose job is to block, so the safe region
    // matters here more than anywhere: a program waiting a minute for a client
    // would otherwise be a collector waiting a minute for the program.
    let done = crate::worker::blocking(|| {
        with_watch(poller, |watch| {
            let ready = watch.poller.wait(timeout)?;
            watch.last.clear();
            for entry in ready {
                // Report the handle W# holds, not the descriptor.
                if let Some((handle, _)) = watch.watched.iter().find(|(_, fd)| *fd == entry.fd) {
                    watch.last.push((*handle, entry.readable, entry.writable));
                }
            }
            Ok(watch.last.len() as i64)
        })
    });
    let result = match done {
        None => FallibleI64::err(no_such_socket()),
        Some(Ok(n)) => FallibleI64::ok(n),
        Some(Err(e)) => FallibleI64::err(sys::error_tag(e)),
    };
    unsafe { out.write(result) };
}

/// The socket the `i`th result of the last wait names.
///
/// # Safety
/// As [`ws_net_connect`].
pub unsafe extern "C" fn ws_net_ready_socket(out: *mut FallibleI64, poller: i64, index: i64) {
    unsafe { crate::gc::checkpoint() };
    let found = with_watch(poller, |watch| {
        usize::try_from(index)
            .ok()
            .and_then(|i| watch.last.get(i))
            .map(|(handle, _, _)| *handle)
    });
    let result = match found.flatten() {
        Some(handle) => FallibleI64::ok(handle),
        None => FallibleI64::err(no_such_socket()),
    };
    unsafe { out.write(result) };
}

/// What the `i`th result is ready for: 1 readable, 2 writable, 3 both.
///
/// A bitmask rather than two calls, because two calls could disagree if a wait
/// happened in between.
///
/// # Safety
/// As [`ws_net_connect`].
pub unsafe extern "C" fn ws_net_ready_events(out: *mut FallibleI64, poller: i64, index: i64) {
    unsafe { crate::gc::checkpoint() };
    let found = with_watch(poller, |watch| {
        usize::try_from(index)
            .ok()
            .and_then(|i| watch.last.get(i))
            .map(|(_, r, w)| i64::from(*r) | (i64::from(*w) << 1))
    });
    let result = match found.flatten() {
        Some(bits) => FallibleI64::ok(bits),
        None => FallibleI64::err(no_such_socket()),
    };
    unsafe { out.write(result) };
}

/// Close a poller. The sockets it was watching are untouched.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary.
pub unsafe extern "C" fn ws_net_close_poller(poller: i64) {
    unsafe { crate::gc::checkpoint() };
    with_pollers(|table| {
        if poller < 0 {
            return;
        }
        if let Some(slot) = table.get_mut(poller as usize) {
            *slot = None;
        }
    });
}

/// Close every socket still open, at exit.
///
/// Not for tidiness -- the process is ending and the kernel would do it -- but
/// so that a listener's port is released before the next test binds it.
pub fn close_all() {
    let entries = with_sockets(std::mem::take);
    for entry in entries.into_iter().flatten() {
        sys::close_socket(entry.fd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A handle names one socket for as long as it is open, and nothing
    /// afterwards.
    #[test]
    fn a_closed_handle_names_nothing() {
        let handle = insert(-1, Kind::Stream);
        assert!(lookup(handle, Kind::Stream).is_some());
        assert!(
            lookup(handle, Kind::Listener).is_none(),
            "a stream is not a listener, and the table says which is which"
        );

        with_sockets(|table| table[handle as usize] = None);
        assert!(lookup(handle, Kind::Stream).is_none());
        assert!(lookup(-1, Kind::Stream).is_none());
        assert!(lookup(i64::MAX, Kind::Stream).is_none());
    }
}
