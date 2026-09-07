//! macOS and the BSDs, declared by hand.
//!
//! Close enough to Linux to look like a copy, and different in exactly the
//! places that would be silent if they were assumed: `errno` is reached through
//! a differently named function, and the `open` flag bits are not the same
//! numbers.

#![allow(non_camel_case_types)]
// `DIR` is what the C header calls it, and a binding whose name does not match
// the reference it was written from is a binding nobody can check.
#![allow(clippy::upper_case_acronyms)]

use super::{Errno, Fd};

pub(crate) type c_int = i32;
/// `mode_t` is 16 bits here and 32 on Linux. It reaches `mkdir` in a register
/// either way, so the width is only ever visible in this declaration -- which
/// is exactly why it should say what the header says.
pub(crate) type mode_t = u16;

/// The directory handle `opendir` answers with, opaque by design.
pub(crate) type DIR = core::ffi::c_void;

/// Where `d_name` starts in a `struct dirent`.
///
/// One number per system rather than a declared struct, for the reason this
/// arm uses `poll(2)` rather than `kqueue`: `struct dirent` is *not the same
/// struct* across this family, and a layout written from the macOS headers
/// would be a declaration for FreeBSD that nobody had ever run. A single
/// offset is one auditable fact per system, taken from that system's
/// `<dirent.h>`, and getting one wrong produces an obviously wrong file name
/// rather than a plausible one. Only macOS is exercised by CI, as with
/// everything else in this file.
#[cfg(any(target_os = "macos", target_os = "ios"))]
const D_NAME_OFFSET: usize = 21; // d_ino, d_seekoff, d_reclen, d_namlen, d_type
#[cfg(any(target_os = "freebsd", target_os = "openbsd"))]
const D_NAME_OFFSET: usize = 24;
#[cfg(target_os = "netbsd")]
const D_NAME_OFFSET: usize = 13;
#[cfg(target_os = "dragonfly")]
const D_NAME_OFFSET: usize = 16;

/// The C library, in a module of its own so that the wrappers below can keep
/// the names the rest of the runtime calls them by.
mod c {
    use super::{DIR, c_int, mode_t};

    unsafe extern "C" {
        /// Variadic because it is: the third argument exists only when `O_CREAT`
        /// is in the flags.
        pub(super) fn open(path: *const u8, flags: c_int, ...) -> c_int;
        pub(super) fn read(fd: c_int, buf: *mut u8, count: usize) -> isize;
        pub(super) fn write(fd: c_int, buf: *const u8, count: usize) -> isize;
        pub(super) fn close(fd: c_int) -> c_int;
        pub(super) fn access(path: *const u8, mode: c_int) -> c_int;
        pub(super) fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
        pub(super) fn unlink(path: *const u8) -> c_int;
        pub(super) fn mkdir(path: *const u8, mode: mode_t) -> c_int;
        pub(super) fn rmdir(path: *const u8) -> c_int;
        pub(super) fn rename(from: *const u8, to: *const u8) -> c_int;
        /// The permission bits only, and `mode_t` is 16 bits here where Linux
        /// has 32 -- the narrowing is done at the call site, on a value already
        /// masked to twelve bits, so it cannot lose anything.
        pub(super) fn chmod(path: *const u8, mode: mode_t) -> c_int;
        /// `off_t` is 64 bits everywhere in this family that the collector's
        /// inline assembly supports.
        pub(super) fn lseek(fd: c_int, offset: i64, whence: c_int) -> i64;
        pub(super) fn closedir(dir: *mut DIR) -> c_int;
        pub(super) fn getenv(name: *const u8) -> *const u8;
        /// Answers with `buf` on success and null on failure, so the path's
        /// length has to be found by looking for the terminator.
        pub(super) fn getcwd(buf: *mut u8, size: usize) -> *mut u8;
        pub(super) fn chdir(path: *const u8) -> c_int;
        /// Replaces this process, so it answers only on failure. `argv` is
        /// null-terminated and its first entry is the program's own name.
        pub(super) fn execvp(file: *const u8, argv: *const *const u8) -> c_int;
    }

    /// How each system in this family answers "what am I?", which is the one
    /// question here with a different shape on every one of them.
    ///
    /// macOS has a libc call for it. FreeBSD, DragonFly and NetBSD have a
    /// `sysctl`, with *different* names for the node -- and NetBSD's is under
    /// `KERN_PROC_ARGS` rather than `KERN_PROC`, with its arguments in another
    /// order. OpenBSD has neither, on purpose: it does not keep the path.
    #[cfg(target_os = "macos")]
    unsafe extern "C" {
        /// Writes a terminated path, and answers -1 with `size` updated to
        /// what it wanted when the buffer was too small. Note that `size` is
        /// in *and* out, and that the path it gives is not resolved -- which
        /// is fine here, since it is used to find a sibling file.
        pub(super) fn _NSGetExecutablePath(buf: *mut u8, size: *mut u32) -> c_int;
    }

    #[cfg(any(target_os = "freebsd", target_os = "dragonfly", target_os = "netbsd"))]
    unsafe extern "C" {
        pub(super) fn sysctl(
            name: *const c_int,
            // `u_int`, which is `u32` on every system in this family.
            namelen: u32,
            old: *mut u8,
            oldlen: *mut usize,
            new: *const u8,
            newlen: usize,
        ) -> c_int;
    }

    // `opendir` and `readdir` are the *one* pair here whose symbol name is not
    // the name in the manual page. macOS carries two directory ABIs -- the
    // original 32-bit-inode one and the 64-bit-inode one every modern SDK
    // compiles against -- and distinguishes them by an `$INODE64` suffix on
    // x86-64 only; arm64 has never had the old ABI, so its symbols are
    // unsuffixed. Linking the unsuffixed name on x86-64 macOS would get the
    // old `struct dirent`, whose `d_name` starts at 8 rather than at 21, and
    // every file name would come back as the tail of some other field.
    unsafe extern "C" {
        #[cfg_attr(
            all(target_os = "macos", target_arch = "x86_64"),
            link_name = "opendir$INODE64"
        )]
        pub(super) fn opendir(path: *const u8) -> *mut DIR;
        /// The pointer is to a `struct dirent` whose layout differs across
        /// this family. Only `d_name` is wanted, and where it starts is
        /// [`D_NAME_OFFSET`].
        #[cfg_attr(
            all(target_os = "macos", target_arch = "x86_64"),
            link_name = "readdir$INODE64"
        )]
        pub(super) fn readdir(dir: *mut DIR) -> *const u8;
        /// Bytes from the kernel's generator. Void return and no length cap,
        /// because it cannot fail: it is present on macOS and on every BSD,
        /// and reseeds itself across a fork. `getentropy` is the alternative
        /// and is capped at 256 bytes a call, which would mean a loop for no
        /// benefit.
        pub(super) fn arc4random_buf(buf: *mut u8, len: usize);
        /// Seconds since the Unix epoch. `time_t` is 64 bits on every member
        /// of this family that this collector's inline assembly supports.
        pub(super) fn time(out: *mut i64) -> i64;
    }

    // `errno`'s address, whose spelling is the one thing here that is not
    // shared across the family.
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "dragonfly"
    ))]
    unsafe extern "C" {
        #[link_name = "__error"]
        pub(super) fn errno_location() -> *mut c_int;
    }

    #[cfg(any(target_os = "netbsd", target_os = "openbsd"))]
    unsafe extern "C" {
        #[link_name = "__errno"]
        pub(super) fn errno_location() -> *mut c_int;
    }
}

const O_RDONLY: c_int = 0;
const O_WRONLY: c_int = 1;
const O_CREAT: c_int = 0x0200;
const O_TRUNC: c_int = 0x0400;
const F_OK: c_int = 0;
/// `access`'s "may this be run?". 1 here and 1 on Linux.
const X_OK: c_int = 1;
const SEEK_END: c_int = 2;

/// Close-on-exec is set afterwards rather than asked for in the flags, because
/// `O_CLOEXEC` is `0x1000000` on macOS and `0x00100000` on FreeBSD -- a
/// difference that would open the file perfectly well with the wrong bit set
/// and never say so. `F_SETFD`/`FD_CLOEXEC` are 2 and 1 everywhere in the
/// family, and everywhere else.
const F_SETFD: c_int = 2;
const FD_CLOEXEC: c_int = 1;

const EPERM: c_int = 1;
const ENOENT: c_int = 2;
const EINTR: c_int = 4;
pub(crate) const EIO: c_int = 5;
const EACCES: c_int = 13;
/// What a `sysctl` answers when the buffer it was given was too small. 12 here
/// and 12 on Linux -- inside the range where the two numberings still agree.
#[cfg(any(target_os = "freebsd", target_os = "dragonfly", target_os = "netbsd"))]
const ENOMEM: c_int = 12;
const EEXIST: c_int = 17;
const ENOTDIR: c_int = 20;
const EISDIR: c_int = 21;
/// "That buffer was too small", which `getcwd` answers with. The same 34 Linux
/// uses -- it is the last code the two numberings agree on.
const ERANGE: c_int = 34;
/// 66 here and 39 on Linux: the numbering agrees only up to 34.
const ENOTEMPTY: c_int = 66;

fn errno() -> Errno {
    Errno(unsafe { *c::errno_location() })
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
        EEXIST => b::ERROR_ALREADY_EXISTS,
        ENOTDIR => b::ERROR_NOT_A_DIRECTORY,
        ENOTEMPTY | EISDIR => b::ERROR_DIRECTORY_NOT_EMPTY,
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

fn set_cloexec(fd: c_int) {
    unsafe { c::fcntl(fd, F_SETFD, FD_CLOEXEC) };
}

/// Bytes from the kernel, and how many arrived -- always all of them.
///
/// The easy arm. `arc4random_buf` has no failure mode to report and no cap to
/// loop around; the shared wrapper's loop simply runs once.
pub(crate) fn random(buf: &mut [u8]) -> Result<usize, Errno> {
    unsafe { c::arc4random_buf(buf.as_mut_ptr(), buf.len()) };
    Ok(buf.len())
}

pub(crate) fn wall_clock_secs() -> i64 {
    unsafe { c::time(std::ptr::null_mut()) }
}

pub(crate) fn open_read(path: &[u8]) -> Result<Fd, Errno> {
    let path = c_path(path)?;
    let fd = unsafe { c::open(path.as_ptr(), O_RDONLY) };
    if fd < 0 {
        return Err(errno());
    }
    set_cloexec(fd);
    Ok(fd as Fd)
}

pub(crate) fn create_write(path: &[u8]) -> Result<Fd, Errno> {
    let path = c_path(path)?;
    // 0o666 as C would write it: the process umask takes it from there, which
    // is what every other tool on the system does.
    let fd = unsafe { c::open(path.as_ptr(), O_WRONLY | O_CREAT | O_TRUNC, 0o666 as c_int) };
    if fd < 0 {
        return Err(errno());
    }
    set_cloexec(fd);
    Ok(fd as Fd)
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
    // The return value is deliberately ignored: a failing `close` has already
    // released the descriptor, so there is nothing a caller could do and
    // retrying would close someone else's file.
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

pub(crate) fn remove(path: &[u8]) -> Result<(), Errno> {
    let path = c_path(path)?;
    if unsafe { c::unlink(path.as_ptr()) } == 0 {
        Ok(())
    } else {
        Err(errno())
    }
}

// ---------------------------------------------------------------------------
// Directories, and the two facts about a path a store needs
// ---------------------------------------------------------------------------

pub(crate) fn mkdir(path: &[u8]) -> Result<(), Errno> {
    let path = c_path(path)?;
    if unsafe { c::mkdir(path.as_ptr(), 0o777) } == 0 {
        Ok(())
    } else {
        Err(errno())
    }
}

pub(crate) fn rmdir(path: &[u8]) -> Result<(), Errno> {
    let path = c_path(path)?;
    if unsafe { c::rmdir(path.as_ptr()) } == 0 {
        Ok(())
    } else {
        Err(errno())
    }
}

pub(crate) fn rename(from: &[u8], to: &[u8]) -> Result<(), Errno> {
    let from = c_path(from)?;
    let to = c_path(to)?;
    if unsafe { c::rename(from.as_ptr(), to.as_ptr()) } == 0 {
        Ok(())
    } else {
        Err(errno())
    }
}

/// Set a path's permission bits.
///
/// `mode` arrives masked to twelve bits, so narrowing it to this family's
/// 16-bit `mode_t` cannot lose anything. The umask does not apply, which is
/// `chmod`'s contract: a caller asking for 0o755 is asking for 0o755.
pub(crate) fn chmod(path: &[u8], mode: u32) -> Result<(), Errno> {
    let path = c_path(path)?;
    if unsafe { c::chmod(path.as_ptr(), mode as mode_t) } == 0 {
        Ok(())
    } else {
        Err(errno())
    }
}

/// Whether this process may run a path.
///
/// `access` rather than a mode read back, for the reason `is_dir` below opens a
/// directory rather than stat'ing one: no `struct stat` in this arm.
pub(crate) fn is_executable(path: &[u8]) -> bool {
    match c_path(path) {
        Ok(path) => (unsafe { c::access(path.as_ptr(), X_OK) }) == 0,
        Err(_) => false,
    }
}

/// Whether a path names a directory, asked by opening it as one.
///
/// No `struct stat` anywhere in this arm, deliberately: that struct has four
/// different layouts across the five systems this file covers, and only one of
/// them is ever run here.
pub(crate) fn is_dir(path: &[u8]) -> bool {
    let Ok(path) = c_path(path) else { return false };
    let dir = unsafe { c::opendir(path.as_ptr()) };
    if dir.is_null() {
        return false;
    }
    unsafe { c::closedir(dir) };
    true
}

/// How many bytes a file holds, by seeking to its end.
pub(crate) fn file_size(path: &[u8]) -> Result<i64, Errno> {
    let fd = open_read(path)?;
    let end = unsafe { c::lseek(fd as c_int, 0, SEEK_END) };
    close(fd);
    if end < 0 { Err(errno()) } else { Ok(end) }
}

pub(crate) fn read_dir(path: &[u8]) -> Result<Vec<Vec<u8>>, Errno> {
    let path = c_path(path)?;
    let dir = unsafe { c::opendir(path.as_ptr()) };
    if dir.is_null() {
        return Err(errno());
    }
    let mut names = Vec::new();
    loop {
        // The end of the directory and a failure are the same null pointer, so
        // `errno` is cleared before the call and read after it.
        unsafe { *c::errno_location() = 0 };
        let entry = unsafe { c::readdir(dir) };
        if entry.is_null() {
            let e = errno();
            unsafe { c::closedir(dir) };
            return if e.0 == 0 { Ok(names) } else { Err(e) };
        }
        let name = unsafe { super::c_string(entry.add(D_NAME_OFFSET)) };
        if name != b"." && name != b".." {
            names.push(name);
        }
    }
}

pub(crate) fn env(name: &[u8]) -> Option<Vec<u8>> {
    let name = c_path(name).ok()?;
    let value = unsafe { c::getenv(name.as_ptr()) };
    if value.is_null() {
        return None;
    }
    Some(unsafe { super::c_string(value) })
}

/// The working directory, or `None` when `room` bytes were not enough.
///
/// The caller asks again with more; see [`super::cwd`] for why the size is not
/// simply `PATH_MAX`.
pub(crate) fn cwd(room: usize) -> Result<Option<Vec<u8>>, Errno> {
    let mut buf = vec![0u8; room];
    if unsafe { c::getcwd(buf.as_mut_ptr(), buf.len()) }.is_null() {
        let e = errno();
        return if e.0 == ERANGE { Ok(None) } else { Err(e) };
    }
    let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    buf.truncate(len);
    Ok(Some(buf))
}

pub(crate) fn chdir(path: &[u8]) -> Result<(), Errno> {
    let path = c_path(path)?;
    if unsafe { c::chdir(path.as_ptr()) } < 0 {
        return Err(errno());
    }
    Ok(())
}

/// Replace this process. Answers only on failure.
pub(crate) fn exec(program: &[u8], argv: &[Vec<u8>]) -> Errno {
    let Ok(program) = c_path(program) else {
        return Errno(ENOENT);
    };
    // The terminated copies must outlive the vector of pointers into them.
    let mut owned: Vec<Vec<u8>> = Vec::with_capacity(argv.len());
    for arg in argv {
        match c_path(arg) {
            Ok(a) => owned.push(a),
            Err(e) => return e,
        }
    }
    let mut pointers: Vec<*const u8> = owned.iter().map(|a| a.as_ptr()).collect();
    pointers.push(core::ptr::null());
    unsafe { c::execvp(program.as_ptr(), pointers.as_ptr()) };
    errno()
}

/// The first level of a `sysctl` name: `CTL_KERN`. The same on every system
/// here, and the only part of the name that is.
#[cfg(any(target_os = "freebsd", target_os = "dragonfly", target_os = "netbsd"))]
const CTL_KERN: c_int = 1;

/// The running executable.
///
/// Four systems, three answers and one refusal -- which is the shape this
/// layer takes everywhere, and why the arms are per system rather than one
/// "BSD" guess.
#[cfg(target_os = "macos")]
pub(crate) fn self_exe(room: usize) -> Result<Option<Vec<u8>>, Errno> {
    let mut buf = vec![0u8; room];
    let mut size = room as u32;
    // -1 means "not enough room", and `size` has been set to how much is
    // wanted -- but the growing loop above will get there anyway, so the
    // number is not read.
    if unsafe { c::_NSGetExecutablePath(buf.as_mut_ptr(), &mut size) } != 0 {
        return Ok(None);
    }
    let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    buf.truncate(len);
    Ok(Some(buf))
}

/// `KERN_PROC` then `KERN_PROC_PATHNAME`, with `-1` meaning "this process".
#[cfg(any(target_os = "freebsd", target_os = "dragonfly"))]
pub(crate) fn self_exe(room: usize) -> Result<Option<Vec<u8>>, Errno> {
    const KERN_PROC: c_int = 14;
    const KERN_PROC_PATHNAME: c_int = 12;
    sysctl_path(&[CTL_KERN, KERN_PROC, KERN_PROC_PATHNAME, -1], room)
}

/// NetBSD keeps it under `KERN_PROC_ARGS`, and puts the pid before the node
/// rather than after it -- so the name is a different shape as well as
/// different numbers.
#[cfg(target_os = "netbsd")]
pub(crate) fn self_exe(room: usize) -> Result<Option<Vec<u8>>, Errno> {
    const KERN_PROC_ARGS: c_int = 48;
    const KERN_PROC_PATHNAME: c_int = 5;
    sysctl_path(&[CTL_KERN, KERN_PROC_ARGS, -1, KERN_PROC_PATHNAME], room)
}

/// OpenBSD does not keep the path of a running program, so there is nothing to
/// ask. Reported rather than approximated from `argv[0]`, which is whatever
/// the caller passed to `exec` and is a bare name for anything found on
/// `PATH`. A caller that wanted a guess can make one; this layer will not.
#[cfg(target_os = "openbsd")]
pub(crate) fn self_exe(_room: usize) -> Result<Option<Vec<u8>>, Errno> {
    Err(Errno(ENOENT))
}

#[cfg(any(target_os = "freebsd", target_os = "dragonfly", target_os = "netbsd"))]
fn sysctl_path(name: &[c_int], room: usize) -> Result<Option<Vec<u8>>, Errno> {
    let mut buf = vec![0u8; room];
    let mut len = room;
    let ok = unsafe {
        c::sysctl(
            name.as_ptr(),
            name.len() as u32,
            buf.as_mut_ptr(),
            &mut len,
            core::ptr::null(),
            0,
        )
    };
    if ok < 0 {
        let e = errno();
        // "Not enough room", which the caller answers by asking for more.
        return if e.0 == ENOMEM { Ok(None) } else { Err(e) };
    }
    // The kernel writes a terminator and counts it; the path is what precedes.
    buf.truncate(len);
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    buf.truncate(end);
    Ok(Some(buf))
}

// ---------------------------------------------------------------------------
// Sockets
// ---------------------------------------------------------------------------

use super::SockAddr;

type socklen_t = u32;

/// `struct addrinfo`. **`ai_canonname` comes before `ai_addr` here and after it
/// on Linux.** Reading one layout through the other yields a pointer that is
/// not an address and does not fault, which is why each arm declares its own
/// rather than sharing one.
#[repr(C)]
struct addrinfo {
    ai_flags: c_int,
    ai_family: c_int,
    ai_socktype: c_int,
    ai_protocol: c_int,
    ai_addrlen: socklen_t,
    ai_canonname: *mut u8,
    ai_addr: *mut u8,
    ai_next: *mut addrinfo,
}

mod net_c {
    use super::{addrinfo, c_int, socklen_t};

    unsafe extern "C" {
        pub(super) fn socket(domain: c_int, ty: c_int, protocol: c_int) -> c_int;
        pub(super) fn connect(fd: c_int, addr: *const u8, len: socklen_t) -> c_int;
        pub(super) fn bind(fd: c_int, addr: *const u8, len: socklen_t) -> c_int;
        pub(super) fn listen(fd: c_int, backlog: c_int) -> c_int;
        pub(super) fn accept(fd: c_int, addr: *mut u8, len: *mut socklen_t) -> c_int;
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
/// Not `1` as on Linux: the BSDs number the socket option levels by protocol
/// and give `SOL_SOCKET` the escape value.
const SOL_SOCKET: c_int = 0xffff;
const SO_REUSEADDR: c_int = 0x0004;
const AI_PASSIVE: c_int = 1;
const F_GETFL: c_int = 3;
const F_SETFL: c_int = 4;
/// `0x4` here and `0o4000` on Linux.
const O_NONBLOCK: c_int = 0x0004;

// The BSD `errno` numbering, which parts company with Linux's above 34.
const EPIPE: c_int = 32;
const EAGAIN: c_int = 35;
const EADDRINUSE: c_int = 48;
const ENETUNREACH: c_int = 51;
const ECONNRESET: c_int = 54;
const ETIMEDOUT: c_int = 60;
const ECONNREFUSED: c_int = 61;
const EHOSTUNREACH: c_int = 65;
/// Not an `errno`: see the Linux arm.
pub(crate) const ERESOLVE: c_int = -1000;

pub(crate) fn socket(addr: &SockAddr) -> Result<Fd, Errno> {
    // No `SOCK_CLOEXEC`: macOS has no such flag, so it is a second call here
    // rather than a bit that exists on some of the family and not others.
    let fd = unsafe { net_c::socket(addr.family(), addr.socktype(), addr.protocol()) };
    if fd < 0 {
        return Err(errno());
    }
    set_cloexec(fd);
    Ok(fd as Fd)
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
    let taken = unsafe { net_c::accept(fd as c_int, core::ptr::null_mut(), core::ptr::null_mut()) };
    if taken < 0 {
        return Err(errno());
    }
    // An accepted socket does not inherit the listener's descriptor flags.
    set_cloexec(taken);
    Ok(taken as Fd)
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
    let flags = unsafe { c::fcntl(fd as c_int, F_GETFL) };
    if flags < 0 {
        return Err(errno());
    }
    let want = if on {
        flags | O_NONBLOCK
    } else {
        flags & !O_NONBLOCK
    };
    if unsafe { c::fcntl(fd as c_int, F_SETFL, want) } < 0 {
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
// Readiness: poll(2)
// ---------------------------------------------------------------------------
//
// `kqueue` and not `poll` is what this family is known for, and what item 8
// named. It is not used here, for a reason worth writing down rather than
// leaving to be rediscovered: **`struct kevent` is not the same struct across
// the family.** FreeBSD 12 added an `ext[4]` tail that macOS does not have, and
// the field widths differ besides. A binding for it can be written from the
// macOS headers and tested on the macOS runner -- and would then be a
// declaration for FreeBSD that nobody has ever run, laid out wrongly, failing
// silently rather than failing to build.
//
// `poll(2)` is POSIX and `struct pollfd` is three fields wide everywhere,
// including Windows, where `WSAPoll` takes the same shape. So the whole family
// gets one implementation that is the same one the tests exercise. What it
// costs is a wait proportional to the number of sockets watched rather than to
// the number ready, which is a scalability concern and not a correctness one:
// `Poller` is the interface, and a `kqueue` implementation can replace this
// without anything above it changing.

#[repr(C)]
#[derive(Clone, Copy)]
struct pollfd {
    fd: c_int,
    events: i16,
    revents: i16,
}

mod poll_c {
    use super::{c_int, pollfd};

    unsafe extern "C" {
        pub(super) fn poll(fds: *mut pollfd, nfds: u32, timeout: c_int) -> c_int;
    }
}

const POLLIN: i16 = 0x0001;
const POLLOUT: i16 = 0x0004;
const POLLERR: i16 = 0x0008;
const POLLHUP: i16 = 0x0010;
const POLLNVAL: i16 = 0x0020;

pub(crate) struct Poller {
    fds: Vec<pollfd>,
}

impl Poller {
    pub(crate) fn new() -> Result<Poller, Errno> {
        Ok(Poller { fds: Vec::new() })
    }

    pub(crate) fn watch(&mut self, fd: Fd, readable: bool, writable: bool) -> Result<(), Errno> {
        let events = (if readable { POLLIN } else { 0 }) | (if writable { POLLOUT } else { 0 });
        match self.fds.iter_mut().find(|p| p.fd == fd as c_int) {
            Some(existing) => existing.events = events,
            None => self.fds.push(pollfd {
                fd: fd as c_int,
                events,
                revents: 0,
            }),
        }
        Ok(())
    }

    pub(crate) fn forget(&mut self, fd: Fd) -> Result<(), Errno> {
        self.fds.retain(|p| p.fd != fd as c_int);
        Ok(())
    }

    pub(crate) fn wait(&mut self, timeout_ms: i32) -> Result<Vec<super::Ready>, Errno> {
        if self.fds.is_empty() {
            // `poll` with no descriptors is a sleep, and a caller waiting on
            // nothing for ever is a caller that has made a mistake.
            return Ok(Vec::new());
        }
        let n = unsafe { poll_c::poll(self.fds.as_mut_ptr(), self.fds.len() as u32, timeout_ms) };
        if n < 0 {
            return Err(errno());
        }
        let mut out = Vec::new();
        for entry in &self.fds {
            let bits = entry.revents;
            if bits == 0 {
                continue;
            }
            // A socket in error, closed, or never valid is reported as ready:
            // the program finds out from the `read` that follows, which is
            // where the failure has a name.
            let broken = bits & (POLLERR | POLLHUP | POLLNVAL) != 0;
            out.push(super::Ready {
                fd: entry.fd as Fd,
                readable: bits & POLLIN != 0 || broken,
                writable: bits & POLLOUT != 0 || broken,
            });
        }
        Ok(out)
    }
}

/// The trust anchors, as a blob of length-prefixed DER certificates.
///
/// macOS keeps them in a keychain rather than in a file, so this is the one
/// place in the family that has an answer. Security.framework hands back a
/// `CFArray` of `SecCertificateRef`, and `SecCertificateCopyData` turns each
/// into the DER this library reads.
///
/// The FreeBSDs go through the file path list in `std/x509` like Linux, so
/// they answer `None`.
#[cfg(target_os = "macos")]
pub(crate) fn system_roots() -> Option<Vec<u8>> {
    // Opaque pointers, all of them: nothing here reads a field of a Core
    // Foundation object, which is what makes declaring them by hand safe.
    type CFTypeRef = *const core::ffi::c_void;
    type CFArrayRef = CFTypeRef;
    type CFDataRef = CFTypeRef;
    type CFIndex = isize;
    type OSStatus = i32;

    #[link(name = "Security", kind = "framework")]
    unsafe extern "C" {
        fn SecTrustCopyAnchorCertificates(certs: *mut CFArrayRef) -> OSStatus;
        fn SecCertificateCopyData(cert: CFTypeRef) -> CFDataRef;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFArrayGetCount(array: CFArrayRef) -> CFIndex;
        fn CFArrayGetValueAtIndex(array: CFArrayRef, index: CFIndex) -> CFTypeRef;
        fn CFDataGetLength(data: CFDataRef) -> CFIndex;
        fn CFDataGetBytePtr(data: CFDataRef) -> *const u8;
        fn CFRelease(object: CFTypeRef);
    }

    let mut anchors: CFArrayRef = core::ptr::null();
    // Copy semantics: this owns the array and every certificate in it, and
    // releases both below.
    if unsafe { SecTrustCopyAnchorCertificates(&mut anchors) } != 0 || anchors.is_null() {
        return None;
    }
    let count = unsafe { CFArrayGetCount(anchors) };
    let mut out = Vec::new();
    for i in 0..count {
        let cert = unsafe { CFArrayGetValueAtIndex(anchors, i) };
        if cert.is_null() {
            continue;
        }
        let data = unsafe { SecCertificateCopyData(cert) };
        if data.is_null() {
            continue;
        }
        let len = unsafe { CFDataGetLength(data) };
        let ptr = unsafe { CFDataGetBytePtr(data) };
        if len > 0 && !ptr.is_null() {
            let der = unsafe { core::slice::from_raw_parts(ptr, len as usize) };
            out.extend_from_slice(&(der.len() as u32).to_be_bytes());
            out.extend_from_slice(der);
        }
        unsafe { CFRelease(data) };
    }
    unsafe { CFRelease(anchors) };
    if out.is_empty() { None } else { Some(out) }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn system_roots() -> Option<Vec<u8>> {
    None
}
