//! Linux, declared by hand.
//!
//! The symbols are the C library's rather than raw `syscall` numbers: the ABI
//! of `open(2)` is stable across architectures in a way the syscall table is
//! not, and glibc and musl both export exactly these names.

#![allow(non_camel_case_types)]

use super::{Errno, Fd};

pub(crate) type c_int = i32;
pub(crate) type c_uint = u32;

/// The C library, in a module of its own so that the wrappers below can
/// keep the names the rest of the runtime calls them by.
mod c {
    use super::{c_int, c_uint};

    unsafe extern "C" {
        /// Variadic because it is: the third argument exists only when `O_CREAT` is
        /// in the flags. Declaring it fixed would be the wrong prototype on any ABI
        /// that passes variadic arguments differently.
        pub(super) fn open(path: *const u8, flags: c_int, ...) -> c_int;
        pub(super) fn read(fd: c_int, buf: *mut u8, count: usize) -> isize;
        pub(super) fn write(fd: c_int, buf: *const u8, count: usize) -> isize;
        pub(super) fn close(fd: c_int) -> c_int;
        pub(super) fn access(path: *const u8, mode: c_int) -> c_int;
        #[cfg(test)]
        pub(super) fn unlink(path: *const u8) -> c_int;
        /// `errno` is a macro in C, and this is what it expands to.
        pub(super) fn __errno_location() -> *mut c_int;
        /// Bytes from the kernel's generator. glibc has exported this since
        /// 2.25 and musl since 1.1.20, which is what keeps it a C library
        /// symbol like everything else here rather than a raw `syscall`.
        pub(super) fn getrandom(buf: *mut u8, len: usize, flags: c_uint) -> isize;
        /// Seconds since the Unix epoch, or -1. One field, so there is no
        /// `struct timespec` to lay out and no 32-bit `time_t` to worry about:
        /// the return is a `time_t`, and on every target this collector
        /// supports that is 64 bits wide.
        pub(super) fn time(out: *mut i64) -> i64;
    }
}

const O_RDONLY: c_int = 0;
const O_WRONLY: c_int = 1;
const O_CREAT: c_int = 0o100;
const O_TRUNC: c_int = 0o1000;
/// So that a spawned worker does not inherit a descriptor it never asked for.
const O_CLOEXEC: c_int = 0o2000000;
const F_OK: c_int = 0;

const EPERM: c_int = 1;
const ENOENT: c_int = 2;
const EINTR: c_int = 4;
pub(crate) const EIO: c_int = 5;
const EACCES: c_int = 13;

/// The last failure on this thread.
fn errno() -> Errno {
    Errno(unsafe { *c::__errno_location() })
}

pub(crate) fn is_interrupted(e: c_int) -> bool {
    e == EINTR
}

pub(crate) fn error_tag(e: c_int) -> i64 {
    use crate::builtins as b;
    match e {
        ENOENT => b::ERROR_NOT_FOUND,
        EACCES | EPERM => b::ERROR_PERMISSION_DENIED,
        ECONNREFUSED => b::ERROR_CONNECTION_REFUSED,
        ECONNRESET => b::ERROR_CONNECTION_RESET,
        EPIPE => b::ERROR_BROKEN_PIPE,
        EADDRINUSE => b::ERROR_ADDRESS_IN_USE,
        ETIMEDOUT => b::ERROR_TIMED_OUT,
        EAGAIN => b::ERROR_WOULD_BLOCK,
        EHOSTUNREACH | ENETUNREACH => b::ERROR_NETWORK_UNREACHABLE,
        ERESOLVE => b::ERROR_HOST_NOT_FOUND,
        _ => b::ERROR_IO_FAILED,
    }
}

/// A W# string is arbitrary bytes; a C path is bytes terminated by a zero.
///
/// A path with a zero inside it is rejected rather than truncated, because
/// truncating would open a *different* file from the one that was named.
fn c_path(path: &[u8]) -> Result<Vec<u8>, Errno> {
    if path.contains(&0) {
        return Err(Errno(ENOENT));
    }
    let mut out = Vec::with_capacity(path.len() + 1);
    out.extend_from_slice(path);
    out.push(0);
    Ok(out)
}

/// Bytes from the kernel, and how many arrived.
///
/// Flags of zero, which means "block until the pool is initialised" -- the
/// only correct choice for a key. It matters exactly once, in the first
/// seconds of a boot, and the alternative is a generator that answers before
/// it has anything to answer with. The blocking is safe here for the reason
/// every other blocking call in this layer is: the caller makes it inside a
/// safe region, so this worker's collector can walk its stack while it waits.
pub(crate) fn random(buf: &mut [u8]) -> Result<usize, Errno> {
    let n = unsafe { c::getrandom(buf.as_mut_ptr(), buf.len(), 0) };
    if n < 0 { Err(errno()) } else { Ok(n as usize) }
}

pub(crate) fn wall_clock_secs() -> i64 {
    unsafe { c::time(std::ptr::null_mut()) }
}

pub(crate) fn open_read(path: &[u8]) -> Result<Fd, Errno> {
    let path = c_path(path)?;
    let fd = unsafe { c::open(path.as_ptr(), O_RDONLY | O_CLOEXEC) };
    if fd < 0 { Err(errno()) } else { Ok(fd as Fd) }
}

pub(crate) fn create_write(path: &[u8]) -> Result<Fd, Errno> {
    let path = c_path(path)?;
    // 0o666 as C would write it: the process umask takes it from there, which
    // is what every other tool on the system does.
    let fd = unsafe {
        c::open(
            path.as_ptr(),
            O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC,
            0o666 as c_int,
        )
    };
    if fd < 0 { Err(errno()) } else { Ok(fd as Fd) }
}

pub(crate) fn read(fd: Fd, buf: &mut [u8]) -> Result<usize, Errno> {
    let n = unsafe { c::read(fd as c_int, buf.as_mut_ptr(), buf.len()) };
    if n < 0 { Err(errno()) } else { Ok(n as usize) }
}

pub(crate) fn write(fd: Fd, buf: &[u8]) -> Result<usize, Errno> {
    let n = unsafe { c::write(fd as c_int, buf.as_ptr(), buf.len()) };
    if n < 0 { Err(errno()) } else { Ok(n as usize) }
}

pub(crate) fn close(fd: Fd) {
    // The return value is deliberately ignored. A failing `close` has already
    // released the descriptor on Linux, so there is nothing a caller could do
    // and retrying would close someone else's file.
    unsafe { c::close(fd as c_int) };
}

pub(crate) fn exists(path: &[u8]) -> bool {
    match c_path(path) {
        Ok(path) => (unsafe { c::access(path.as_ptr(), F_OK) }) == 0,
        Err(_) => false,
    }
}

pub(crate) fn stdin() -> Fd {
    0
}

#[cfg(test)]
pub(crate) fn remove(path: &[u8]) -> Result<(), Errno> {
    let path = c_path(path)?;
    if unsafe { c::unlink(path.as_ptr()) } == 0 {
        Ok(())
    } else {
        Err(errno())
    }
}

// ---------------------------------------------------------------------------
// Sockets
// ---------------------------------------------------------------------------

use super::SockAddr;

type socklen_t = u32;

/// `struct addrinfo`, whose *field order is not the same on every platform*:
/// Linux puts `ai_addr` before `ai_canonname`, and the BSDs and Windows put
/// them the other way round. Reading one layout through the other yields a
/// pointer that is not an address and does not fault, which is the worst kind
/// of wrong. Each arm therefore declares its own.
#[repr(C)]
struct addrinfo {
    ai_flags: c_int,
    ai_family: c_int,
    ai_socktype: c_int,
    ai_protocol: c_int,
    ai_addrlen: socklen_t,
    ai_addr: *mut u8,
    ai_canonname: *mut u8,
    ai_next: *mut addrinfo,
}

mod net_c {
    use super::{addrinfo, c_int, socklen_t};

    unsafe extern "C" {
        pub(super) fn socket(domain: c_int, ty: c_int, protocol: c_int) -> c_int;
        pub(super) fn connect(fd: c_int, addr: *const u8, len: socklen_t) -> c_int;
        pub(super) fn bind(fd: c_int, addr: *const u8, len: socklen_t) -> c_int;
        pub(super) fn listen(fd: c_int, backlog: c_int) -> c_int;
        /// The `4` is Linux's: it takes the flags that would otherwise need a
        /// second `fcntl`, and an accepted socket that is not close-on-exec is
        /// one a later `exec` leaks.
        pub(super) fn accept4(fd: c_int, addr: *mut u8, len: *mut socklen_t, flags: c_int)
        -> c_int;
        pub(super) fn send(fd: c_int, buf: *const u8, len: usize, flags: c_int) -> isize;
        pub(super) fn recv(fd: c_int, buf: *mut u8, len: usize, flags: c_int) -> isize;
        pub(super) fn sendto(
            fd: c_int,
            buf: *const u8,
            len: usize,
            flags: c_int,
            addr: *const u8,
            addrlen: socklen_t,
        ) -> isize;
        pub(super) fn recvfrom(
            fd: c_int,
            buf: *mut u8,
            len: usize,
            flags: c_int,
            addr: *mut u8,
            addrlen: *mut socklen_t,
        ) -> isize;
        pub(super) fn setsockopt(
            fd: c_int,
            level: c_int,
            name: c_int,
            value: *const u8,
            len: socklen_t,
        ) -> c_int;
        pub(super) fn getsockname(fd: c_int, addr: *mut u8, len: *mut socklen_t) -> c_int;
        pub(super) fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
        pub(super) fn getaddrinfo(
            node: *const u8,
            service: *const u8,
            hints: *const addrinfo,
            res: *mut *mut addrinfo,
        ) -> c_int;
        pub(super) fn freeaddrinfo(res: *mut addrinfo);
    }
}

const AF_UNSPEC: c_int = 0;
const SOCK_STREAM: c_int = 1;
const SOCK_DGRAM: c_int = 2;
const SOCK_CLOEXEC: c_int = 0o2000000;
const SOL_SOCKET: c_int = 1;
const SO_REUSEADDR: c_int = 2;
const AI_PASSIVE: c_int = 1;
const F_GETFL: c_int = 3;
const F_SETFL: c_int = 4;
const O_NONBLOCK: c_int = 0o4000;

const EPIPE: c_int = 32;
const EAGAIN: c_int = 11;
const ENETUNREACH: c_int = 101;
const ECONNRESET: c_int = 104;
const ETIMEDOUT: c_int = 110;
const ECONNREFUSED: c_int = 111;
const EHOSTUNREACH: c_int = 113;
const EADDRINUSE: c_int = 98;
/// Not an `errno`. `getaddrinfo` reports `EAI_*` codes from a numbering of its
/// own, which on glibc is negative and on macOS is small and positive -- so
/// rather than pass either through, a failure to resolve is reported as this,
/// which no `errno` can be.
pub(crate) const ERESOLVE: c_int = -1000;

pub(crate) fn socket(addr: &SockAddr) -> Result<Fd, Errno> {
    let fd = unsafe {
        net_c::socket(
            addr.family(),
            addr.socktype() | SOCK_CLOEXEC,
            addr.protocol(),
        )
    };
    if fd < 0 { Err(errno()) } else { Ok(fd as Fd) }
}

pub(crate) fn connect(fd: Fd, addr: &SockAddr) -> Result<(), Errno> {
    if unsafe { net_c::connect(fd as c_int, addr.as_ptr(), addr.len()) } == 0 {
        Ok(())
    } else {
        Err(errno())
    }
}

pub(crate) fn bind(fd: Fd, addr: &SockAddr) -> Result<(), Errno> {
    if unsafe { net_c::bind(fd as c_int, addr.as_ptr(), addr.len()) } == 0 {
        Ok(())
    } else {
        Err(errno())
    }
}

pub(crate) fn listen(fd: Fd, backlog: i32) -> Result<(), Errno> {
    if unsafe { net_c::listen(fd as c_int, backlog) } == 0 {
        Ok(())
    } else {
        Err(errno())
    }
}

pub(crate) fn accept(fd: Fd) -> Result<Fd, Errno> {
    let taken = unsafe {
        net_c::accept4(
            fd as c_int,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            SOCK_CLOEXEC,
        )
    };
    if taken < 0 {
        Err(errno())
    } else {
        Ok(taken as Fd)
    }
}

pub(crate) fn send(fd: Fd, buf: &[u8]) -> Result<usize, Errno> {
    let n = unsafe { net_c::send(fd as c_int, buf.as_ptr(), buf.len(), 0) };
    if n < 0 { Err(errno()) } else { Ok(n as usize) }
}

pub(crate) fn recv(fd: Fd, buf: &mut [u8]) -> Result<usize, Errno> {
    let n = unsafe { net_c::recv(fd as c_int, buf.as_mut_ptr(), buf.len(), 0) };
    if n < 0 { Err(errno()) } else { Ok(n as usize) }
}

pub(crate) fn send_to(fd: Fd, addr: &SockAddr, buf: &[u8]) -> Result<usize, Errno> {
    let n = unsafe {
        net_c::sendto(
            fd as c_int,
            buf.as_ptr(),
            buf.len(),
            0,
            addr.as_ptr(),
            addr.len(),
        )
    };
    if n < 0 { Err(errno()) } else { Ok(n as usize) }
}

pub(crate) fn recv_from(fd: Fd, buf: &mut [u8]) -> Result<(usize, SockAddr), Errno> {
    let mut from = [0u8; 128];
    let mut len: socklen_t = from.len() as socklen_t;
    let n = unsafe {
        net_c::recvfrom(
            fd as c_int,
            buf.as_mut_ptr(),
            buf.len(),
            0,
            from.as_mut_ptr(),
            &mut len,
        )
    };
    if n < 0 {
        return Err(errno());
    }
    // The family is carried so the address can be sent back to; the socket type
    // is a datagram by construction, since only a datagram socket receives one.
    let peer = unsafe { SockAddr::from_raw(from.as_ptr(), len, AF_UNSPEC, SOCK_DGRAM, 0) };
    Ok((n as usize, peer))
}

pub(crate) fn set_reuse_addr(fd: Fd) -> Result<(), Errno> {
    let on: c_int = 1;
    let ok = unsafe {
        net_c::setsockopt(
            fd as c_int,
            SOL_SOCKET,
            SO_REUSEADDR,
            (&raw const on) as *const u8,
            size_of::<c_int>() as socklen_t,
        )
    };
    if ok == 0 { Ok(()) } else { Err(errno()) }
}

pub(crate) fn set_nonblocking(fd: Fd, on: bool) -> Result<(), Errno> {
    let flags = unsafe { net_c::fcntl(fd as c_int, F_GETFL) };
    if flags < 0 {
        return Err(errno());
    }
    let want = if on {
        flags | O_NONBLOCK
    } else {
        flags & !O_NONBLOCK
    };
    if unsafe { net_c::fcntl(fd as c_int, F_SETFL, want) } < 0 {
        Err(errno())
    } else {
        Ok(())
    }
}

pub(crate) fn local_addr(fd: Fd) -> Result<SockAddr, Errno> {
    let mut bytes = [0u8; 128];
    let mut len: socklen_t = bytes.len() as socklen_t;
    if unsafe { net_c::getsockname(fd as c_int, bytes.as_mut_ptr(), &mut len) } != 0 {
        return Err(errno());
    }
    // The family is not asked for here and not needed: nothing creates a socket
    // from this address, it is only read for its port.
    Ok(unsafe { SockAddr::from_raw(bytes.as_ptr(), len, AF_UNSPEC, 0, 0) })
}

pub(crate) fn close_socket(fd: Fd) {
    close(fd);
}

pub(crate) fn resolve(
    host: &[u8],
    port: u16,
    stream: bool,
    passive: bool,
) -> Result<Vec<SockAddr>, Errno> {
    let host = if host.is_empty() {
        None
    } else {
        Some(c_path(host)?)
    };
    let service = format!("{port}\0");
    let hints = addrinfo {
        // `AI_NUMERICSERV` would say "the service is a number, do not consult
        // /etc/services" -- true here, but it is `0x400` on Linux, `0x1000` on
        // macOS and `0x8` on FreeBSD, and a resolver given the wrong flag does
        // something else silently. A decimal service string parses as a number
        // without being told to.
        ai_flags: if passive { AI_PASSIVE } else { 0 },
        ai_family: AF_UNSPEC,
        ai_socktype: if stream { SOCK_STREAM } else { SOCK_DGRAM },
        ai_protocol: 0,
        ai_addrlen: 0,
        ai_addr: core::ptr::null_mut(),
        ai_canonname: core::ptr::null_mut(),
        ai_next: core::ptr::null_mut(),
    };
    let mut head: *mut addrinfo = core::ptr::null_mut();
    let rc = unsafe {
        net_c::getaddrinfo(
            host.as_ref().map_or(core::ptr::null(), |h| h.as_ptr()),
            service.as_ptr(),
            &hints,
            &mut head,
        )
    };
    if rc != 0 {
        return Err(Errno(ERESOLVE));
    }
    let mut out = Vec::new();
    let mut cursor = head;
    while !cursor.is_null() {
        let entry = unsafe { &*cursor };
        if !entry.ai_addr.is_null() {
            out.push(unsafe {
                SockAddr::from_raw(
                    entry.ai_addr,
                    entry.ai_addrlen,
                    entry.ai_family,
                    entry.ai_socktype,
                    entry.ai_protocol,
                )
            });
        }
        cursor = entry.ai_next;
    }
    unsafe { net_c::freeaddrinfo(head) };
    if out.is_empty() {
        return Err(Errno(ERESOLVE));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Readiness: epoll
// ---------------------------------------------------------------------------

/// **`epoll_event` is packed on x86-64 and not on any other architecture.**
///
/// x86-64 packs it so that the 32-bit and 64-bit layouts agree, which nothing
/// else needed to do; on aarch64 it is an ordinary `#[repr(C)]` struct with the
/// natural padding. Getting this wrong shifts `data` by four bytes and hands
/// back a file descriptor that is a piece of one, which is not a failure the
/// kernel can report.
#[repr(C)]
#[cfg_attr(target_arch = "x86_64", repr(packed))]
#[derive(Clone, Copy)]
struct epoll_event {
    events: u32,
    data: u64,
}

mod poll_c {
    use super::{c_int, epoll_event};

    unsafe extern "C" {
        pub(super) fn epoll_create1(flags: c_int) -> c_int;
        pub(super) fn epoll_ctl(
            epfd: c_int,
            op: c_int,
            fd: c_int,
            event: *mut epoll_event,
        ) -> c_int;
        pub(super) fn epoll_wait(
            epfd: c_int,
            events: *mut epoll_event,
            max: c_int,
            timeout: c_int,
        ) -> c_int;
    }
}

const EPOLL_CLOEXEC: c_int = 0o2000000;
const EPOLL_CTL_ADD: c_int = 1;
const EPOLL_CTL_DEL: c_int = 2;
const EPOLL_CTL_MOD: c_int = 3;
const EPOLLIN: u32 = 0x001;
const EPOLLOUT: u32 = 0x004;
const EPOLLERR: u32 = 0x008;
const EPOLLHUP: u32 = 0x010;

/// How many ready sockets one `epoll_wait` may report. More than this simply
/// come back on the next call.
const MAX_EVENTS: usize = 128;

pub(crate) struct Poller {
    epfd: c_int,
    /// Which descriptors are in the set, so that watching one twice is a
    /// modification rather than the error the kernel would otherwise report.
    watched: Vec<Fd>,
}

impl Poller {
    pub(crate) fn new() -> Result<Poller, Errno> {
        let epfd = unsafe { poll_c::epoll_create1(EPOLL_CLOEXEC) };
        if epfd < 0 {
            return Err(errno());
        }
        Ok(Poller {
            epfd,
            watched: Vec::new(),
        })
    }

    pub(crate) fn watch(&mut self, fd: Fd, readable: bool, writable: bool) -> Result<(), Errno> {
        let mut event = epoll_event {
            events: (if readable { EPOLLIN } else { 0 }) | (if writable { EPOLLOUT } else { 0 }),
            data: fd as u64,
        };
        let known = self.watched.contains(&fd);
        let op = if known { EPOLL_CTL_MOD } else { EPOLL_CTL_ADD };
        if unsafe { poll_c::epoll_ctl(self.epfd, op, fd as c_int, &mut event) } != 0 {
            return Err(errno());
        }
        if !known {
            self.watched.push(fd);
        }
        Ok(())
    }

    pub(crate) fn forget(&mut self, fd: Fd) -> Result<(), Errno> {
        // The event pointer is unused for a delete, but kernels before 2.6.9
        // insisted on one and passing a valid pointer costs nothing.
        let mut event = epoll_event { events: 0, data: 0 };
        let removed =
            unsafe { poll_c::epoll_ctl(self.epfd, EPOLL_CTL_DEL, fd as c_int, &mut event) };
        self.watched.retain(|&w| w != fd);
        if removed != 0 { Err(errno()) } else { Ok(()) }
    }

    pub(crate) fn wait(&mut self, timeout_ms: i32) -> Result<Vec<super::Ready>, Errno> {
        let mut events = [epoll_event { events: 0, data: 0 }; MAX_EVENTS];
        let n = unsafe {
            poll_c::epoll_wait(
                self.epfd,
                events.as_mut_ptr(),
                MAX_EVENTS as c_int,
                timeout_ms,
            )
        };
        if n < 0 {
            return Err(errno());
        }
        let mut out = Vec::with_capacity(n as usize);
        for event in events.iter().take(n as usize) {
            // Copied out whole: the struct is packed on x86-64, where taking a
            // reference to a field would be unaligned.
            let event = *event;
            let bits = event.events;
            // A socket in error, or one whose far end has gone, is reported as
            // readable: the program finds out from the `read` that follows,
            // which is where the error has a name.
            let broken = bits & (EPOLLERR | EPOLLHUP) != 0;
            out.push(super::Ready {
                fd: event.data as Fd,
                readable: bits & EPOLLIN != 0 || broken,
                writable: bits & EPOLLOUT != 0 || broken,
            });
        }
        Ok(out)
    }
}

impl Drop for Poller {
    fn drop(&mut self) {
        close(self.epfd as Fd);
    }
}

/// The trust anchors, as a blob of length-prefixed DER certificates.
///
/// `None` here, and that is the whole of the Linux arm: there is no system
/// call and no library that answers this question. Every distribution ships a
/// concatenated PEM bundle instead, at one of a handful of paths, and
/// `std/x509` tries them with `io.exists` and `io.read_file` -- which is why
/// this platform needed no new syscall at all.
pub(crate) fn system_roots() -> Option<Vec<u8>> {
    None
}
