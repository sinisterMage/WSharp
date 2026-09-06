//! Windows, declared by hand.
//!
//! A third arm rather than a variation on the others, because there is no libc
//! here worth targeting. Files are `CreateFileW`/`ReadFile`/`WriteFile`, paths
//! are UTF-16, and the error code comes from `GetLastError` rather than from a
//! thread-local `int` the C library owns.

// The Win32 names are kept exactly as the platform documentation spells them.
// A binding whose name does not match the reference it was written from is a
// binding nobody can check.
#![allow(non_camel_case_types)]
#![allow(clippy::upper_case_acronyms)]

use super::{Errno, Fd};

type c_void = core::ffi::c_void;
type HANDLE = *mut c_void;
type BOOL = i32;
type DWORD = u32;

// The Win32 calls this arm makes.
//
// `extern "system"` rather than `extern "C"`: they coincide on x86-64 and
// aarch64, and do not on 32-bit x86, where the Win32 ABI is `stdcall`.
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateFileW(
        path: *const u16,
        access: DWORD,
        share: DWORD,
        security: *mut c_void,
        disposition: DWORD,
        flags: DWORD,
        template: HANDLE,
    ) -> HANDLE;
    fn ReadFile(
        file: HANDLE,
        buf: *mut u8,
        to_read: DWORD,
        read: *mut DWORD,
        overlapped: *mut c_void,
    ) -> BOOL;
    fn WriteFile(
        file: HANDLE,
        buf: *const u8,
        to_write: DWORD,
        written: *mut DWORD,
        overlapped: *mut c_void,
    ) -> BOOL;
    fn CloseHandle(handle: HANDLE) -> BOOL;
    fn GetFileAttributesW(path: *const u16) -> DWORD;
    fn GetStdHandle(which: DWORD) -> HANDLE;
    fn GetLastError() -> DWORD;
    #[cfg(test)]
    fn DeleteFileW(path: *const u16) -> BOOL;
}

const GENERIC_READ: DWORD = 0x8000_0000;
const GENERIC_WRITE: DWORD = 0x4000_0000;
const FILE_SHARE_READ: DWORD = 0x0000_0001;
const FILE_SHARE_WRITE: DWORD = 0x0000_0002;
const FILE_SHARE_DELETE: DWORD = 0x0000_0004;
const CREATE_ALWAYS: DWORD = 2;
const OPEN_EXISTING: DWORD = 3;
const FILE_ATTRIBUTE_NORMAL: DWORD = 0x0000_0080;
const INVALID_FILE_ATTRIBUTES: DWORD = DWORD::MAX;
const STD_INPUT_HANDLE: DWORD = -10i32 as DWORD;

const ERROR_FILE_NOT_FOUND: i32 = 2;
const ERROR_PATH_NOT_FOUND: i32 = 3;
const ERROR_ACCESS_DENIED: i32 = 5;
/// Reading the far end of a closed pipe. Unix calls that end of input, and so
/// does everything above this module, so it is translated rather than reported.
const ERROR_BROKEN_PIPE: i32 = 109;
/// What a `write` that made no progress reports. There is no `EIO` here.
pub(crate) const EIO: i32 = 31; // ERROR_GEN_FAILURE

/// `INVALID_HANDLE_VALUE`, which is -1 rather than null -- and null is a
/// perfectly ordinary failure return from some other calls, so the two are not
/// interchangeable.
fn invalid_handle() -> HANDLE {
    -1isize as HANDLE
}

fn last_error() -> Errno {
    Errno(unsafe { GetLastError() } as i32)
}

/// Windows has no `EINTR`: a blocking call is not cut short by a signal.
pub(crate) fn is_interrupted(_e: i32) -> bool {
    false
}

pub(crate) fn error_tag(e: i32) -> i64 {
    // The socket codes live in a range of their own (10000 and up), so file
    // errors and Winsock errors cannot be confused for one another -- which is
    // exactly why they can share one function here.
    if let Some(tag) = socket_error_tag(e) {
        return tag;
    }
    match e {
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => crate::builtins::ERROR_NOT_FOUND,
        ERROR_ACCESS_DENIED => crate::builtins::ERROR_PERMISSION_DENIED,
        ERROR_BROKEN_PIPE => crate::builtins::ERROR_BROKEN_PIPE,
        _ => crate::builtins::ERROR_IO_FAILED,
    }
}

/// A W# string is arbitrary bytes and a Win32 path is UTF-16.
///
/// The bytes are taken as UTF-8, which is what every W# string literal is and
/// what every path a program builds out of them will be. A zero inside the path
/// is rejected rather than truncated, for the same reason it is on Unix.
fn wide_path(path: &[u8]) -> Result<Vec<u16>, Errno> {
    if path.contains(&0) {
        return Err(Errno(ERROR_FILE_NOT_FOUND));
    }
    let text = String::from_utf8_lossy(path);
    let mut out: Vec<u16> = text.encode_utf16().collect();
    out.push(0);
    Ok(out)
}

pub(crate) fn open_read(path: &[u8]) -> Result<Fd, Errno> {
    let path = wide_path(path)?;
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            core::ptr::null_mut(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            core::ptr::null_mut(),
        )
    };
    if handle == invalid_handle() {
        return Err(last_error());
    }
    Ok(handle as Fd)
}

pub(crate) fn create_write(path: &[u8]) -> Result<Fd, Errno> {
    let path = wide_path(path)?;
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ,
            core::ptr::null_mut(),
            // Truncates an existing file and creates one that is not there,
            // which is what `write_file` means.
            CREATE_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            core::ptr::null_mut(),
        )
    };
    if handle == invalid_handle() {
        return Err(last_error());
    }
    Ok(handle as Fd)
}

/// Win32 counts in `DWORD`s, so a request longer than 4 GiB is made in pieces.
/// The caller already loops, so returning a short count is all this has to do.
fn clamp(len: usize) -> DWORD {
    len.min(DWORD::MAX as usize) as DWORD
}

pub(crate) fn read(fd: Fd, buf: &mut [u8]) -> Result<usize, Errno> {
    let mut read: DWORD = 0;
    let ok = unsafe {
        ReadFile(
            fd as HANDLE,
            buf.as_mut_ptr(),
            clamp(buf.len()),
            &mut read,
            core::ptr::null_mut(),
        )
    };
    if ok == 0 {
        let e = last_error();
        // The far end of a pipe has gone. Everything above this speaks Unix,
        // where that is simply the end of the stream.
        if e.0 == ERROR_BROKEN_PIPE {
            return Ok(0);
        }
        return Err(e);
    }
    Ok(read as usize)
}

pub(crate) fn write(fd: Fd, buf: &[u8]) -> Result<usize, Errno> {
    let mut written: DWORD = 0;
    let ok = unsafe {
        WriteFile(
            fd as HANDLE,
            buf.as_ptr(),
            clamp(buf.len()),
            &mut written,
            core::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(last_error());
    }
    Ok(written as usize)
}

pub(crate) fn close(fd: Fd) {
    // Standard handles are not ours to close, and closing one would take
    // standard input away from the rest of the program.
    if fd == 0 || fd == invalid_handle() as Fd {
        return;
    }
    unsafe { CloseHandle(fd as HANDLE) };
}

pub(crate) fn exists(path: &[u8]) -> bool {
    match wide_path(path) {
        Ok(path) => (unsafe { GetFileAttributesW(path.as_ptr()) }) != INVALID_FILE_ATTRIBUTES,
        Err(_) => false,
    }
}

pub(crate) fn stdin() -> Fd {
    (unsafe { GetStdHandle(STD_INPUT_HANDLE) }) as Fd
}

#[cfg(test)]
pub(crate) fn remove(path: &[u8]) -> Result<(), Errno> {
    let path = wide_path(path)?;
    if unsafe { DeleteFileW(path.as_ptr()) } != 0 {
        Ok(())
    } else {
        Err(last_error())
    }
}

// ---------------------------------------------------------------------------
// Sockets
// ---------------------------------------------------------------------------

use super::SockAddr;
use std::sync::Once;

/// A Windows `SOCKET` is a `UINT_PTR`, **not a file descriptor and not a
/// `HANDLE`**: it is closed with `closesocket` rather than `CloseHandle`, and
/// read with `recv` rather than `ReadFile`. Mixing them up mostly works and
/// then does not.
type SOCKET = usize;
const INVALID_SOCKET: SOCKET = usize::MAX;

/// `struct addrinfo`. `ai_canonname` comes before `ai_addr` as on the BSDs, and
/// `ai_addrlen` is a `size_t` rather than the `socklen_t` every Unix uses.
#[repr(C)]
struct addrinfo {
    ai_flags: i32,
    ai_family: i32,
    ai_socktype: i32,
    ai_protocol: i32,
    ai_addrlen: usize,
    ai_canonname: *mut u8,
    ai_addr: *mut u8,
    ai_next: *mut addrinfo,
}

mod net_c {
    use super::{SOCKET, addrinfo};

    #[link(name = "ws2_32")]
    unsafe extern "system" {
        pub(super) fn WSAStartup(version: u16, data: *mut u8) -> i32;
        pub(super) fn WSAGetLastError() -> i32;
        pub(super) fn socket(af: i32, ty: i32, protocol: i32) -> SOCKET;
        pub(super) fn connect(s: SOCKET, addr: *const u8, len: i32) -> i32;
        pub(super) fn bind(s: SOCKET, addr: *const u8, len: i32) -> i32;
        pub(super) fn listen(s: SOCKET, backlog: i32) -> i32;
        pub(super) fn accept(s: SOCKET, addr: *mut u8, len: *mut i32) -> SOCKET;
        pub(super) fn send(s: SOCKET, buf: *const u8, len: i32, flags: i32) -> i32;
        pub(super) fn recv(s: SOCKET, buf: *mut u8, len: i32, flags: i32) -> i32;
        pub(super) fn sendto(
            s: SOCKET,
            buf: *const u8,
            len: i32,
            flags: i32,
            addr: *const u8,
            addrlen: i32,
        ) -> i32;
        pub(super) fn recvfrom(
            s: SOCKET,
            buf: *mut u8,
            len: i32,
            flags: i32,
            addr: *mut u8,
            addrlen: *mut i32,
        ) -> i32;
        pub(super) fn getsockname(s: SOCKET, addr: *mut u8, len: *mut i32) -> i32;
        pub(super) fn ioctlsocket(s: SOCKET, cmd: i32, arg: *mut u32) -> i32;
        pub(super) fn closesocket(s: SOCKET) -> i32;
        pub(super) fn getaddrinfo(
            node: *const u8,
            service: *const u8,
            hints: *const addrinfo,
            res: *mut *mut addrinfo,
        ) -> i32;
        pub(super) fn freeaddrinfo(res: *mut addrinfo);
    }
}

const AF_UNSPEC: i32 = 0;
const SOCK_STREAM: i32 = 1;
const SOCK_DGRAM: i32 = 2;
const AI_PASSIVE: i32 = 1;
/// `FIONBIO`, the only way to make a Windows socket non-blocking: there is no
/// `fcntl` here.
const FIONBIO: i32 = -2147195266; // 0x8004667E as a signed int

const WSAEWOULDBLOCK: i32 = 10035;
const WSAENETUNREACH: i32 = 10051;
const WSAECONNABORTED: i32 = 10053;
const WSAECONNRESET: i32 = 10054;
const WSAESHUTDOWN: i32 = 10058;
const WSAETIMEDOUT: i32 = 10060;
const WSAECONNREFUSED: i32 = 10061;
const WSAEHOSTUNREACH: i32 = 10065;
const WSAEADDRINUSE: i32 = 10048;
const WSAHOST_NOT_FOUND: i32 = 11001;
const WSANO_DATA: i32 = 11004;
/// Not a `WSAGetLastError` code: see the Linux arm.
pub(crate) const ERESOLVE: i32 = -1000;

/// Winsock must be started before any of it is used, and it is reference
/// counted, so starting it once for the process and never stopping is the
/// simplest correct thing. `WSADATA` is only ever written, never read: 512
/// bytes is comfortably larger than any version of it.
fn ensure_winsock() {
    static STARTED: Once = Once::new();
    STARTED.call_once(|| {
        let mut data = [0u8; 512];
        // 2.2, as every program since 1996 has asked for.
        unsafe { net_c::WSAStartup(0x0202, data.as_mut_ptr()) };
    });
}

fn wsa_error() -> Errno {
    Errno(unsafe { net_c::WSAGetLastError() })
}

pub(crate) fn socket_error_tag(e: i32) -> Option<i64> {
    use crate::builtins as b;
    Some(match e {
        WSAECONNREFUSED => b::ERROR_CONNECTION_REFUSED,
        WSAECONNRESET | WSAECONNABORTED => b::ERROR_CONNECTION_RESET,
        WSAESHUTDOWN => b::ERROR_BROKEN_PIPE,
        WSAEADDRINUSE => b::ERROR_ADDRESS_IN_USE,
        WSAETIMEDOUT => b::ERROR_TIMED_OUT,
        WSAEWOULDBLOCK => b::ERROR_WOULD_BLOCK,
        WSAEHOSTUNREACH | WSAENETUNREACH => b::ERROR_NETWORK_UNREACHABLE,
        WSAHOST_NOT_FOUND | WSANO_DATA | ERESOLVE => b::ERROR_HOST_NOT_FOUND,
        _ => return None,
    })
}

fn to_socket(fd: Fd) -> SOCKET {
    fd as SOCKET
}

pub(crate) fn socket(addr: &SockAddr) -> Result<Fd, Errno> {
    ensure_winsock();
    let s = unsafe { net_c::socket(addr.family(), addr.socktype(), addr.protocol()) };
    if s == INVALID_SOCKET {
        Err(wsa_error())
    } else {
        Ok(s as Fd)
    }
}

pub(crate) fn connect(fd: Fd, addr: &SockAddr) -> Result<(), Errno> {
    if unsafe { net_c::connect(to_socket(fd), addr.as_ptr(), addr.len() as i32) } == 0 {
        Ok(())
    } else {
        Err(wsa_error())
    }
}

pub(crate) fn bind(fd: Fd, addr: &SockAddr) -> Result<(), Errno> {
    if unsafe { net_c::bind(to_socket(fd), addr.as_ptr(), addr.len() as i32) } == 0 {
        Ok(())
    } else {
        Err(wsa_error())
    }
}

pub(crate) fn listen(fd: Fd, backlog: i32) -> Result<(), Errno> {
    if unsafe { net_c::listen(to_socket(fd), backlog) } == 0 {
        Ok(())
    } else {
        Err(wsa_error())
    }
}

pub(crate) fn accept(fd: Fd) -> Result<Fd, Errno> {
    let taken =
        unsafe { net_c::accept(to_socket(fd), core::ptr::null_mut(), core::ptr::null_mut()) };
    if taken == INVALID_SOCKET {
        Err(wsa_error())
    } else {
        Ok(taken as Fd)
    }
}

/// Winsock counts in `int`s, so a request larger than 2 GiB is made in pieces.
/// The caller already loops, so a short count is all this has to return.
fn clamp_i32(len: usize) -> i32 {
    len.min(i32::MAX as usize) as i32
}

pub(crate) fn send(fd: Fd, buf: &[u8]) -> Result<usize, Errno> {
    let n = unsafe { net_c::send(to_socket(fd), buf.as_ptr(), clamp_i32(buf.len()), 0) };
    if n < 0 {
        Err(wsa_error())
    } else {
        Ok(n as usize)
    }
}

pub(crate) fn recv(fd: Fd, buf: &mut [u8]) -> Result<usize, Errno> {
    let n = unsafe { net_c::recv(to_socket(fd), buf.as_mut_ptr(), clamp_i32(buf.len()), 0) };
    if n < 0 {
        Err(wsa_error())
    } else {
        Ok(n as usize)
    }
}

pub(crate) fn send_to(fd: Fd, addr: &SockAddr, buf: &[u8]) -> Result<usize, Errno> {
    let n = unsafe {
        net_c::sendto(
            to_socket(fd),
            buf.as_ptr(),
            clamp_i32(buf.len()),
            0,
            addr.as_ptr(),
            addr.len() as i32,
        )
    };
    if n < 0 {
        Err(wsa_error())
    } else {
        Ok(n as usize)
    }
}

pub(crate) fn recv_from(fd: Fd, buf: &mut [u8]) -> Result<(usize, SockAddr), Errno> {
    let mut from = [0u8; 128];
    let mut len: i32 = from.len() as i32;
    let n = unsafe {
        net_c::recvfrom(
            to_socket(fd),
            buf.as_mut_ptr(),
            clamp_i32(buf.len()),
            0,
            from.as_mut_ptr(),
            &mut len,
        )
    };
    if n < 0 {
        return Err(wsa_error());
    }
    let peer = unsafe { SockAddr::from_raw(from.as_ptr(), len as u32, AF_UNSPEC, SOCK_DGRAM, 0) };
    Ok((n as usize, peer))
}

/// Deliberately nothing.
///
/// `SO_REUSEADDR` does not mean here what it means on Unix: instead of allowing
/// a restart over a port still in `TIME_WAIT`, it lets a second socket bind a
/// port another socket is *actively listening on*, and the two then receive
/// each other's connections. Windows does not need the Unix workaround, so the
/// right port of it is to do nothing rather than to set the same-named flag.
pub(crate) fn set_reuse_addr(_fd: Fd) -> Result<(), Errno> {
    Ok(())
}

pub(crate) fn set_nonblocking(fd: Fd, on: bool) -> Result<(), Errno> {
    let mut arg: u32 = u32::from(on);
    if unsafe { net_c::ioctlsocket(to_socket(fd), FIONBIO, &mut arg) } == 0 {
        Ok(())
    } else {
        Err(wsa_error())
    }
}

pub(crate) fn local_addr(fd: Fd) -> Result<SockAddr, Errno> {
    let mut bytes = [0u8; 128];
    let mut len: i32 = bytes.len() as i32;
    if unsafe { net_c::getsockname(to_socket(fd), bytes.as_mut_ptr(), &mut len) } != 0 {
        return Err(wsa_error());
    }
    Ok(unsafe { SockAddr::from_raw(bytes.as_ptr(), len as u32, AF_UNSPEC, 0, 0) })
}

pub(crate) fn close_socket(fd: Fd) {
    // `closesocket`, not `CloseHandle`: a `SOCKET` is not a `HANDLE`.
    unsafe { net_c::closesocket(to_socket(fd)) };
}

pub(crate) fn resolve(
    host: &[u8],
    port: u16,
    stream: bool,
    passive: bool,
) -> Result<Vec<SockAddr>, Errno> {
    ensure_winsock();
    if host.contains(&0) {
        return Err(Errno(ERESOLVE));
    }
    let host_c: Option<Vec<u8>> = if host.is_empty() {
        None
    } else {
        let mut v = host.to_vec();
        v.push(0);
        Some(v)
    };
    let service = format!("{port}\0");
    let hints = addrinfo {
        ai_flags: if passive { AI_PASSIVE } else { 0 },
        ai_family: AF_UNSPEC,
        ai_socktype: if stream { SOCK_STREAM } else { SOCK_DGRAM },
        ai_protocol: 0,
        ai_addrlen: 0,
        ai_canonname: core::ptr::null_mut(),
        ai_addr: core::ptr::null_mut(),
        ai_next: core::ptr::null_mut(),
    };
    let mut head: *mut addrinfo = core::ptr::null_mut();
    let rc = unsafe {
        net_c::getaddrinfo(
            host_c.as_ref().map_or(core::ptr::null(), |h| h.as_ptr()),
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
                    entry.ai_addrlen as u32,
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
// Readiness: WSAPoll
// ---------------------------------------------------------------------------
//
// Item 8 named IOCP. `WSAPoll` is used instead because it is *readiness*-based
// like `epoll` and `poll`, where IOCP is completion-based: adopting it would
// push a second model -- buffers handed to the kernel and handed back later --
// through every layer above, for a scalability win nothing here needs yet.
// `WSAPOLLFD` is `struct pollfd` by another name, so this arm is the BSD one
// with different flag numbers.
//
// One documented quirk: `WSAPoll` does not report a *failed* connect as
// writable, so a program using it to wait for an outgoing connection would wait
// out its timeout instead of being told. Nothing here connects that way --
// `connect` blocks inside a safe region -- but it is the reason not to build
// one on this later without checking.

#[repr(C)]
#[derive(Clone, Copy)]
struct WSAPOLLFD {
    fd: SOCKET,
    events: i16,
    revents: i16,
}

mod poll_c {
    use super::{SOCKET, WSAPOLLFD};

    #[link(name = "ws2_32")]
    unsafe extern "system" {
        pub(super) fn WSAPoll(fds: *mut WSAPOLLFD, nfds: u32, timeout: i32) -> i32;
    }

    // Kept so the import above is obviously about sockets.
    #[allow(dead_code)]
    pub(super) type Handle = SOCKET;
}

// Winsock's numbering, which is not the Unix one.
const POLLRDNORM: i16 = 0x0100;
const POLLWRNORM: i16 = 0x0010;
const POLLERR: i16 = 0x0001;
const POLLHUP: i16 = 0x0002;
const POLLNVAL: i16 = 0x0004;

pub(crate) struct Poller {
    fds: Vec<WSAPOLLFD>,
}

impl Poller {
    pub(crate) fn new() -> Result<Poller, Errno> {
        ensure_winsock();
        Ok(Poller { fds: Vec::new() })
    }

    pub(crate) fn watch(&mut self, fd: Fd, readable: bool, writable: bool) -> Result<(), Errno> {
        let events =
            (if readable { POLLRDNORM } else { 0 }) | (if writable { POLLWRNORM } else { 0 });
        let s = to_socket(fd);
        match self.fds.iter_mut().find(|p| p.fd == s) {
            Some(existing) => existing.events = events,
            None => self.fds.push(WSAPOLLFD {
                fd: s,
                events,
                revents: 0,
            }),
        }
        Ok(())
    }

    pub(crate) fn forget(&mut self, fd: Fd) -> Result<(), Errno> {
        let s = to_socket(fd);
        self.fds.retain(|p| p.fd != s);
        Ok(())
    }

    pub(crate) fn wait(&mut self, timeout_ms: i32) -> Result<Vec<super::Ready>, Errno> {
        if self.fds.is_empty() {
            return Ok(Vec::new());
        }
        let n =
            unsafe { poll_c::WSAPoll(self.fds.as_mut_ptr(), self.fds.len() as u32, timeout_ms) };
        if n < 0 {
            return Err(wsa_error());
        }
        let mut out = Vec::new();
        for entry in &self.fds {
            let bits = entry.revents;
            if bits == 0 {
                continue;
            }
            let broken = bits & (POLLERR | POLLHUP | POLLNVAL) != 0;
            out.push(super::Ready {
                fd: entry.fd as Fd,
                readable: bits & POLLRDNORM != 0 || broken,
                writable: bits & POLLWRNORM != 0 || broken,
            });
        }
        Ok(out)
    }
}
