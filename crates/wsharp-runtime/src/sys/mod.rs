//! The operating system, declared by hand.
//!
//! `std/io` used to go through Rust's `std::fs` and `std::io`, which was the
//! right trade while the library was four functions. It stops being the right
//! trade the moment networking arrives, for three reasons:
//!
//! * **Flags and error codes are not reachable.** `std::fs::read` opens a file
//!   one way; `O_NONBLOCK` and `O_CLOEXEC` are not expressible, and
//!   `std::io::ErrorKind` is a portable approximation where `errno` is the real
//!   answer.
//! * **Networking needs a readiness API** -- `epoll` on Linux, `kqueue` on the
//!   BSDs and macOS, `WSAPoll` on Windows. None of that is in `std`, so the
//!   socket half would be hand-written regardless; doing the file half the same
//!   way keeps one layer rather than two.
//! * **A blocking syscall is a hole in the safepoint protocol.** That one is
//!   answered above this module, by `worker::blocking`: every call here is made
//!   inside a safe region, so the collector can pause a thread that is waiting
//!   on the world.
//!
//! **Hand-declared bindings, not the `libc` crate.** `wsharp-runtime` has zero
//! dependencies and that is worth more than a few dozen `extern "C"`
//! declarations are worth avoiding: it is the crate both the type checker and
//! the code generator depend on, and it is what keeps the build offline.
//!
//! **Three arms, not a `#[cfg(unix)]` split.** Windows has no libc worth
//! targeting -- `CreateFileW`/`ReadFile`, and WSA for sockets -- and pretending
//! otherwise would put the difference in the wrong place. Each arm is named for
//! what it is, and each declares only what it needs to: everything that can be
//! written once is written here instead, which is also what keeps the arms this
//! box cannot run honest, since only this file is exercised by the tests.
//!
//! Portability is now ours. `std::fs` handled path encoding, retry on `EINTR`,
//! short reads and the difference between a file and a pipe; each of those is a
//! thing to get right, per platform, below.

#[cfg(target_os = "linux")]
pub(crate) mod linux;
#[cfg(target_os = "linux")]
pub(crate) use linux as imp;

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]
pub(crate) mod bsd;
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]
pub(crate) use bsd as imp;

#[cfg(target_os = "windows")]
pub(crate) mod windows;
#[cfg(target_os = "windows")]
pub(crate) use windows as imp;

/// An open file or socket, as W# sees it.
///
/// A signed word because that is what a W# `i64` handle is, and because every
/// platform's failure value is negative or -1: a Unix file descriptor is an
/// `int`, a Windows `HANDLE` is a pointer-sized value whose invalid form is -1.
pub(crate) type Fd = i64;

/// Why a call failed, in the platform's own numbering.
///
/// Kept raw rather than classified at the point of failure: `errno` is the real
/// answer, and the one place that needs a W# error name can ask for one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Errno(pub i32);

impl Errno {
    /// Whether the call was cut short by a signal and should simply be made
    /// again. `std::fs` used to absorb this; now it is ours.
    pub(crate) fn is_interrupted(self) -> bool {
        imp::is_interrupted(self.0)
    }
}

/// A generic failure, for the few places that need an error and have none of
/// their own: a loop over candidate addresses that somehow saw none.
pub(crate) fn io_failed() -> Errno {
    Errno(imp::EIO)
}

/// The W# error name a failure reports, as a tag.
///
/// Only the distinctions a program can act on. The mapping is per arm because
/// the numbers are: `ENOENT` is 2 on Unix and `ERROR_FILE_NOT_FOUND` is also 2
/// on Windows, but that is a coincidence and nothing should rely on it.
pub(crate) fn error_tag(e: Errno) -> i64 {
    imp::error_tag(e.0)
}

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

/// Read the whole of a file.
///
/// A loop rather than one call, because a `read` may return short for reasons
/// that are not the end of the file -- a pipe, a signal, a slow device.
pub(crate) fn read_file(path: &[u8]) -> Result<Vec<u8>, Errno> {
    let fd = imp::open_read(path)?;
    let out = read_to_end(fd);
    imp::close(fd);
    out
}

/// Replace a file's contents.
pub(crate) fn write_file(path: &[u8], bytes: &[u8]) -> Result<(), Errno> {
    let fd = imp::create_write(path)?;
    let out = write_all(fd, bytes);
    imp::close(fd);
    out
}

pub(crate) fn exists(path: &[u8]) -> bool {
    imp::exists(path)
}

/// Everything left on `fd`.
pub(crate) fn read_to_end(fd: Fd) -> Result<Vec<u8>, Errno> {
    let mut out = Vec::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        match read(fd, &mut buf) {
            Ok(0) => return Ok(out),
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(e) => return Err(e),
        }
    }
}

/// Write all of `bytes`, however many calls that takes.
pub(crate) fn write_all(fd: Fd, bytes: &[u8]) -> Result<(), Errno> {
    let mut written = 0;
    while written < bytes.len() {
        match write(fd, &bytes[written..]) {
            // A zero-length write is not progress; treating it as success would
            // spin here for ever.
            Ok(0) => return Err(Errno(imp::EIO)),
            Ok(n) => written += n,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// One `read`, retried if a signal cut it short.
pub(crate) fn read(fd: Fd, buf: &mut [u8]) -> Result<usize, Errno> {
    loop {
        match imp::read(fd, buf) {
            Err(e) if e.is_interrupted() => continue,
            other => return other,
        }
    }
}

/// One `write`, retried if a signal cut it short.
pub(crate) fn write(fd: Fd, buf: &[u8]) -> Result<usize, Errno> {
    loop {
        match imp::write(fd, buf) {
            Err(e) if e.is_interrupted() => continue,
            other => return other,
        }
    }
}

/// Standard input.
pub(crate) fn stdin() -> Fd {
    imp::stdin()
}

/// One line from `fd`, without its terminator; `None` at end of input.
///
/// Read a byte at a time on purpose. A buffered read would consume past the
/// newline, and this descriptor is shared with whatever the program does with
/// standard input next -- the line is the unit the caller asked for, so it is
/// the unit taken from the stream. Lines are short and this is a terminal.
pub(crate) fn read_line(fd: Fd) -> Result<Option<Vec<u8>>, Errno> {
    let mut out = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match read(fd, &mut byte)? {
            0 if out.is_empty() => return Ok(None),
            0 => break,
            _ if byte[0] == b'\n' => break,
            _ => out.push(byte[0]),
        }
    }
    // A file written on Windows ends its lines "\r\n" wherever it is read.
    if out.last() == Some(&b'\r') {
        out.pop();
    }
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> Vec<u8> {
        let mut dir = std::env::temp_dir();
        dir.push(format!("wsharp-sys-{}-{}", std::process::id(), name));
        dir.to_string_lossy().into_owned().into_bytes()
    }

    #[test]
    fn a_file_written_by_hand_reads_back_by_hand() {
        let path = temp_path("roundtrip");
        assert!(!exists(&path), "the temporary file is not there yet");

        // Deliberately not UTF-8: a W# `str` is bytes, and the file half must
        // not quietly assume otherwise.
        let contents: Vec<u8> = vec![0, 1, 2, 0xff, b'h', b'i', b'\n', 0x80];
        write_file(&path, &contents).expect("written");
        assert!(exists(&path));
        assert_eq!(read_file(&path).expect("read back"), contents);

        // Replacing shortens as well as lengthens.
        write_file(&path, b"short").expect("rewritten");
        assert_eq!(read_file(&path).expect("read back"), b"short");

        let _ = imp::remove(&path);
        assert!(!exists(&path), "and it is gone again");
    }

    #[test]
    fn reading_a_file_that_is_not_there_says_so() {
        let path = temp_path("absent");
        let err = read_file(&path).expect_err("there is no such file");
        assert_eq!(
            error_tag(err),
            crate::builtins::ERROR_NOT_FOUND,
            "errno {} was not recognised as `NotFound`",
            err.0
        );
    }

    /// A file larger than the read buffer, so the loop in `read_to_end` runs
    /// more than once -- which is the part `std::fs::read` used to do for us.
    #[test]
    fn a_file_larger_than_the_buffer_is_read_whole() {
        let path = temp_path("large");
        let contents: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        write_file(&path, &contents).expect("written");
        assert_eq!(read_file(&path).expect("read back"), contents);
        let _ = imp::remove(&path);
    }

    #[test]
    fn lines_are_split_and_their_terminators_removed() {
        let path = temp_path("lines");
        write_file(&path, b"one\r\ntwo\n\nthree").expect("written");
        let fd = imp::open_read(&path).expect("opened");
        let mut lines = Vec::new();
        while let Some(line) = read_line(fd).expect("read") {
            lines.push(line);
        }
        imp::close(fd);
        let _ = imp::remove(&path);
        assert_eq!(
            lines,
            vec![
                b"one".to_vec(),
                b"two".to_vec(),
                b"".to_vec(),
                b"three".to_vec()
            ],
            "a blank line is a line, and a file need not end with a newline"
        );
    }
}

// ---------------------------------------------------------------------------
// Sockets
// ---------------------------------------------------------------------------

/// One address a name resolved to, in the platform's own bytes.
///
/// Never built here. `getaddrinfo` produces these and everything else consumes
/// them, so no `sockaddr_in` is laid out by hand and no port number is
/// byte-swapped by hand -- both easy to get wrong, and silent when wrong. It
/// also means an IPv6 address costs nothing extra: it is simply a longer one.
#[derive(Clone, Copy)]
pub(crate) struct SockAddr {
    /// `sockaddr_storage`, which is 128 bytes on all three platforms.
    bytes: [u8; 128],
    len: u32,
    /// Carried rather than read back out of `bytes`, because `sockaddr` begins
    /// with a `u16` family on Linux and Windows and with a `u8` length then a
    /// `u8` family on the BSDs. The resolver already knows; asking it twice in
    /// two different layouts is how that difference goes unnoticed.
    family: i32,
    socktype: i32,
    protocol: i32,
}

/// The bytes are not worth printing and the shape is: a 128-byte dump would
/// bury the two fields anyone reading a failure wants.
impl std::fmt::Debug for SockAddr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "SockAddr(family {}, port {}, {} bytes)",
            self.family,
            self.port(),
            self.len
        )
    }
}

impl SockAddr {
    /// # Safety
    /// `ptr` must point at `len` readable bytes of a `sockaddr`.
    pub(crate) unsafe fn from_raw(
        ptr: *const u8,
        len: u32,
        family: i32,
        socktype: i32,
        protocol: i32,
    ) -> SockAddr {
        let mut bytes = [0u8; 128];
        let len = len.min(bytes.len() as u32);
        unsafe { std::ptr::copy_nonoverlapping(ptr, bytes.as_mut_ptr(), len as usize) };
        SockAddr {
            bytes,
            len,
            family,
            socktype,
            protocol,
        }
    }

    pub(crate) fn as_ptr(&self) -> *const u8 {
        self.bytes.as_ptr()
    }

    pub(crate) fn len(&self) -> u32 {
        self.len
    }

    pub(crate) fn family(&self) -> i32 {
        self.family
    }

    pub(crate) fn socktype(&self) -> i32 {
        self.socktype
    }

    pub(crate) fn protocol(&self) -> i32 {
        self.protocol
    }

    /// The port, in host order.
    ///
    /// This one *is* portable, and unusually so: `sin_port` and `sin6_port` are
    /// both at offset 2 in network order on every platform here, because the
    /// BSDs' extra `sin_len` byte took the space the wider family field uses
    /// elsewhere.
    pub(crate) fn port(&self) -> u16 {
        u16::from_be_bytes([self.bytes[2], self.bytes[3]])
    }
}

/// Every address `host:port` resolves to, most preferred first.
///
/// `passive` asks for an address to bind to rather than to connect to, which is
/// what turns an empty host into "every interface".
pub(crate) fn resolve(
    host: &[u8],
    port: u16,
    stream: bool,
    passive: bool,
) -> Result<Vec<SockAddr>, Errno> {
    imp::resolve(host, port, stream, passive)
}

/// Connect to the first address that will have us.
///
/// Trying each in turn is what makes a dual-stack host work: a name commonly
/// resolves to an IPv6 address this machine cannot reach and an IPv4 address it
/// can, and only trying tells them apart. The last failure is the one reported,
/// because it is the one for the address most likely to have been right.
pub(crate) fn tcp_connect(addrs: &[SockAddr]) -> Result<Fd, Errno> {
    let mut last = io_failed();
    for addr in addrs {
        let fd = match imp::socket(addr) {
            Ok(fd) => fd,
            Err(e) => {
                last = e;
                continue;
            }
        };
        match imp::connect(fd, addr) {
            Ok(()) => return Ok(fd),
            Err(e) => {
                imp::close_socket(fd);
                last = e;
            }
        }
    }
    Err(last)
}

/// Bind and listen on the first address that will have us.
pub(crate) fn tcp_listen(addrs: &[SockAddr], backlog: i32) -> Result<Fd, Errno> {
    let mut last = io_failed();
    for addr in addrs {
        let fd = match imp::socket(addr) {
            Ok(fd) => fd,
            Err(e) => {
                last = e;
                continue;
            }
        };
        // Without this a server cannot be restarted until its last connections
        // have finished lingering, which in a test suite means the next test.
        let bound = imp::set_reuse_addr(fd).and_then(|()| imp::bind(fd, addr));
        match bound.and_then(|()| imp::listen(fd, backlog)) {
            Ok(()) => return Ok(fd),
            Err(e) => {
                imp::close_socket(fd);
                last = e;
            }
        }
    }
    Err(last)
}

/// Take the next waiting connection.
pub(crate) fn accept(fd: Fd) -> Result<Fd, Errno> {
    loop {
        match imp::accept(fd) {
            Err(e) if e.is_interrupted() => continue,
            other => return other,
        }
    }
}

pub(crate) fn send(fd: Fd, buf: &[u8]) -> Result<usize, Errno> {
    loop {
        match imp::send(fd, buf) {
            Err(e) if e.is_interrupted() => continue,
            other => return other,
        }
    }
}

pub(crate) fn recv(fd: Fd, buf: &mut [u8]) -> Result<usize, Errno> {
    loop {
        match imp::recv(fd, buf) {
            Err(e) if e.is_interrupted() => continue,
            other => return other,
        }
    }
}

/// Bind a datagram socket to the first address that will have us.
///
/// The same `resolve` as TCP, asked for datagrams instead -- which is why UDP
/// costs so little here: the address it needs is the same address.
pub(crate) fn udp_bind(addrs: &[SockAddr]) -> Result<Fd, Errno> {
    let mut last = io_failed();
    for addr in addrs {
        let fd = match imp::socket(addr) {
            Ok(fd) => fd,
            Err(e) => {
                last = e;
                continue;
            }
        };
        match imp::bind(fd, addr) {
            Ok(()) => return Ok(fd),
            Err(e) => {
                imp::close_socket(fd);
                last = e;
            }
        }
    }
    Err(last)
}

/// One datagram, to one address. A datagram is sent whole or not at all, so
/// there is no short-write loop here as there is for a stream.
pub(crate) fn send_to(fd: Fd, addr: &SockAddr, buf: &[u8]) -> Result<usize, Errno> {
    loop {
        match imp::send_to(fd, addr, buf) {
            Err(e) if e.is_interrupted() => continue,
            other => return other,
        }
    }
}

/// One datagram, and where it came from.
///
/// A datagram longer than `buf` is truncated and the rest is lost, which is
/// what UDP does: the caller chooses the largest message it is prepared to
/// receive.
pub(crate) fn recv_from(fd: Fd, buf: &mut [u8]) -> Result<(usize, SockAddr), Errno> {
    loop {
        match imp::recv_from(fd, buf) {
            Err(e) if e.is_interrupted() => continue,
            other => return other,
        }
    }
}

pub(crate) fn set_nonblocking(fd: Fd, on: bool) -> Result<(), Errno> {
    imp::set_nonblocking(fd, on)
}

/// The address this socket is actually bound to, which is how a server that
/// asked for port 0 finds out which port it got.
pub(crate) fn local_addr(fd: Fd) -> Result<SockAddr, Errno> {
    imp::local_addr(fd)
}

pub(crate) fn close_socket(fd: Fd) {
    imp::close_socket(fd);
}

#[cfg(test)]
mod socket_tests {
    use super::*;

    /// A whole TCP conversation over the loopback interface, with the syscalls
    /// this module declares rather than `std::net`.
    ///
    /// Binding to port 0 and asking what was granted is deliberate: a hardcoded
    /// port makes a test that fails when the machine happens to be using it,
    /// and every W# case that follows uses the same trick.
    #[test]
    fn a_tcp_conversation_over_loopback() {
        let bind_to = resolve(b"127.0.0.1", 0, true, true).expect("loopback resolves");
        let server = tcp_listen(&bind_to, 8).expect("bound and listening");
        let port = local_addr(server).expect("bound somewhere").port();
        assert_ne!(
            port, 0,
            "port 0 means the kernel chose one, and it says which"
        );

        let client = std::thread::spawn(move || {
            let to = resolve(b"127.0.0.1", port, true, false).expect("resolves");
            let fd = tcp_connect(&to).expect("connects");
            assert_eq!(send(fd, b"ping").expect("sent"), 4);
            let mut buf = [0u8; 64];
            let n = recv(fd, &mut buf).expect("received");
            close_socket(fd);
            buf[..n].to_vec()
        });

        let taken = accept(server).expect("a connection arrived");
        let mut buf = [0u8; 64];
        let n = recv(taken, &mut buf).expect("received");
        assert_eq!(&buf[..n], b"ping");
        assert_eq!(send(taken, b"pong").expect("sent"), 4);
        close_socket(taken);
        close_socket(server);

        assert_eq!(client.join().expect("the client finished"), b"pong");
    }

    /// Nothing is listening on a port nothing was bound to, and the failure has
    /// a name a program can act on.
    #[test]
    fn connecting_to_nothing_is_refused() {
        // Bind, ask which port, then give it up: that port was free a moment
        // ago and is the one nothing is listening on now.
        let bind_to = resolve(b"127.0.0.1", 0, true, true).expect("resolves");
        let server = tcp_listen(&bind_to, 1).expect("bound");
        let port = local_addr(server).expect("bound somewhere").port();
        close_socket(server);

        let to = resolve(b"127.0.0.1", port, true, false).expect("resolves");
        let err = tcp_connect(&to).expect_err("nothing is listening");
        assert_eq!(
            error_tag(err),
            crate::builtins::ERROR_CONNECTION_REFUSED,
            "errno {} was not recognised as a refused connection",
            err.0
        );
    }

    #[test]
    fn a_name_that_does_not_resolve_says_so() {
        let err = resolve(b"no-such-host.invalid", 80, true, false)
            .expect_err("`.invalid` is reserved never to resolve");
        assert_eq!(error_tag(err), crate::builtins::ERROR_HOST_NOT_FOUND);
    }

    /// A non-blocking socket with nothing to read answers immediately, and the
    /// answer is `WouldBlock` rather than a failure.
    #[test]
    fn a_non_blocking_socket_says_when_there_is_nothing_yet() {
        let bind_to = resolve(b"127.0.0.1", 0, true, true).expect("resolves");
        let server = tcp_listen(&bind_to, 1).expect("bound");
        set_nonblocking(server, true).expect("set");
        let err = accept(server).expect_err("nobody has connected");
        assert_eq!(
            error_tag(err),
            crate::builtins::ERROR_WOULD_BLOCK,
            "errno {} was not recognised as `would block`",
            err.0
        );
        close_socket(server);
    }
}

// ---------------------------------------------------------------------------
// Readiness
// ---------------------------------------------------------------------------

/// What one wait reports about one socket.
///
/// `readable` covers the end of the stream as well as data arriving: a socket
/// whose far end has gone is readable, and reading it is how a program finds
/// out. An error on the socket is reported the same way, so that the program
/// learns about it from the `read` that follows rather than from a second
/// mechanism here.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Ready {
    pub(crate) fd: Fd,
    pub(crate) readable: bool,
    pub(crate) writable: bool,
}

/// A set of sockets to wait on.
///
/// The interface is deliberately the smallest one a readiness loop needs --
/// watch, forget, wait -- so that the implementation underneath can be replaced
/// per platform without anything above noticing.
pub(crate) struct Poller(imp::Poller);

impl Poller {
    pub(crate) fn new() -> Result<Poller, Errno> {
        imp::Poller::new().map(Poller)
    }

    /// Watch `fd`, or change what it is watched for. Watching for nothing is
    /// how a socket is kept in the set while it has nothing to say.
    pub(crate) fn watch(&mut self, fd: Fd, readable: bool, writable: bool) -> Result<(), Errno> {
        self.0.watch(fd, readable, writable)
    }

    pub(crate) fn forget(&mut self, fd: Fd) -> Result<(), Errno> {
        self.0.forget(fd)
    }

    /// Wait until something is ready, or `timeout_ms` passes. A negative
    /// timeout waits indefinitely; zero polls and returns at once.
    ///
    /// The caller is expected to be inside a safe region: this is the one call
    /// in the whole layer whose *job* is to block.
    pub(crate) fn wait(&mut self, timeout_ms: i32) -> Result<Vec<Ready>, Errno> {
        loop {
            match self.0.wait(timeout_ms) {
                Err(e) if e.is_interrupted() => continue,
                other => return other,
            }
        }
    }
}
