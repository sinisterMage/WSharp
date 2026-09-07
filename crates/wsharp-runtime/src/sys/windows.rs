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
// Structure fields and the enumeration values passed as arguments keep their
// documented spelling too, for the same reason the function names do.
#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]
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
    fn DeleteFileW(path: *const u16) -> BOOL;
    fn CreateDirectoryW(path: *const u16, security: *mut c_void) -> BOOL;
    fn RemoveDirectoryW(path: *const u16) -> BOOL;
    fn MoveFileExW(from: *const u16, to: *const u16, flags: DWORD) -> BOOL;
    fn GetFileAttributesExW(path: *const u16, level: DWORD, info: *mut c_void) -> BOOL;
    fn FindFirstFileW(pattern: *const u16, data: *mut WIN32_FIND_DATAW) -> HANDLE;
    fn FindNextFileW(handle: HANDLE, data: *mut WIN32_FIND_DATAW) -> BOOL;
    fn FindClose(handle: HANDLE) -> BOOL;
    fn GetEnvironmentVariableW(name: *const u16, buf: *mut u16, size: DWORD) -> DWORD;
    fn GetCurrentDirectoryW(size: DWORD, buf: *mut u16) -> DWORD;
}

/// What `GetFileAttributesExW` fills in at `GetFileExInfoStandard`.
///
/// Declared where the POSIX `struct stat` is not, and the difference is the
/// point: this struct's layout is documented, fixed, and the same on every
/// Windows, while `struct stat` differs by system and by architecture. It is
/// the one call that answers both questions this layer asks about a path.
#[repr(C)]
struct WIN32_FILE_ATTRIBUTE_DATA {
    dwFileAttributes: DWORD,
    ftCreationTime: FILETIME,
    ftLastAccessTime: FILETIME,
    ftLastWriteTime: FILETIME,
    nFileSizeHigh: DWORD,
    nFileSizeLow: DWORD,
}

/// What a directory walk hands back. `cAlternateFileName` is the 8.3 name and
/// is never read here, but it is part of the struct the caller must supply.
#[repr(C)]
struct WIN32_FIND_DATAW {
    dwFileAttributes: DWORD,
    ftCreationTime: FILETIME,
    ftLastAccessTime: FILETIME,
    ftLastWriteTime: FILETIME,
    nFileSizeHigh: DWORD,
    nFileSizeLow: DWORD,
    dwReserved0: DWORD,
    dwReserved1: DWORD,
    cFileName: [u16; 260],
    cAlternateFileName: [u16; 14],
}

const GetFileExInfoStandard: DWORD = 0;
const MOVEFILE_REPLACE_EXISTING: DWORD = 0x0000_0001;
const FILE_ATTRIBUTE_DIRECTORY: DWORD = 0x0000_0010;

// Bytes from the system's generator. Neither kernel32 nor ws2_32 has it, so
// this is a third library -- and `BCryptGenRandom` is the documented modern
// entry point. `RtlGenRandom` is the older alternative and is reached by the
// ordinal name `SystemFunction036`, which Microsoft has never documented.
#[link(name = "bcrypt")]
unsafe extern "system" {
    fn BCryptGenRandom(algorithm: *mut c_void, buf: *mut u8, len: DWORD, flags: DWORD) -> NTSTATUS;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetSystemTimeAsFileTime(out: *mut FILETIME);
}

/// Not a `BOOL` and not an `errno`: zero is success and anything else is a
/// status code that `GetLastError` knows nothing about.
type NTSTATUS = i32;

/// 100-nanosecond ticks since 1601-01-01, split in two because the struct is.
#[repr(C)]
struct FILETIME {
    low: DWORD,
    high: DWORD,
}

/// Ask for the system-preferred algorithm, which is what lets the handle be
/// null and saves opening one.
const BCRYPT_USE_SYSTEM_PREFERRED_RNG: DWORD = 0x0000_0002;

/// 1601-01-01 to 1970-01-01, in the 100-nanosecond ticks a `FILETIME` counts.
const FILETIME_EPOCH_DELTA: u64 = 116_444_736_000_000_000;
const FILETIME_TICKS_PER_SECOND: u64 = 10_000_000;

/// Bytes from the system, and how many arrived -- always all of them, since
/// this either fills the buffer or fails.
///
/// A single call is capped at a `DWORD`'s worth, which the shared wrapper's
/// loop takes care of. The status is not an `errno`, so it cannot go through
/// `error_tag`: a failure here is reported as plain I/O failure, which is the
/// truth -- there is nothing a program could do differently for one code
/// rather than another.
pub(crate) fn random(buf: &mut [u8]) -> Result<usize, Errno> {
    let want = buf.len().min(DWORD::MAX as usize);
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            buf.as_mut_ptr(),
            want as DWORD,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status == 0 {
        Ok(want)
    } else {
        Err(Errno(EIO))
    }
}

pub(crate) fn wall_clock_secs() -> i64 {
    let mut ft = FILETIME { low: 0, high: 0 };
    unsafe { GetSystemTimeAsFileTime(&raw mut ft) };
    let ticks = (u64::from(ft.high) << 32) | u64::from(ft.low);
    // Before the Unix epoch the subtraction would wrap, which a clock set to
    // 1600 could produce. Saturating there gives a date the caller will reject
    // rather than one far in the future that it will not.
    ticks.saturating_sub(FILETIME_EPOCH_DELTA) as i64 / FILETIME_TICKS_PER_SECOND as i64
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
const ERROR_FILE_EXISTS: i32 = 80;
const ERROR_DIR_NOT_EMPTY: i32 = 145;
const ERROR_ALREADY_EXISTS: i32 = 183;
/// "The directory name is invalid" -- what Windows says where Unix says
/// `ENOTDIR`.
const ERROR_DIRECTORY: i32 = 267;
const ERROR_NO_MORE_FILES: i32 = 18;

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
        ERROR_FILE_EXISTS | ERROR_ALREADY_EXISTS => crate::builtins::ERROR_ALREADY_EXISTS,
        ERROR_DIRECTORY => crate::builtins::ERROR_NOT_A_DIRECTORY,
        ERROR_DIR_NOT_EMPTY => crate::builtins::ERROR_DIRECTORY_NOT_EMPTY,
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

pub(crate) fn remove(path: &[u8]) -> Result<(), Errno> {
    let path = wide_path(path)?;
    if unsafe { DeleteFileW(path.as_ptr()) } != 0 {
        Ok(())
    } else {
        Err(last_error())
    }
}

// ---------------------------------------------------------------------------
// Directories, and the two facts about a path a store needs
// ---------------------------------------------------------------------------

/// UTF-16 back to the bytes everything above this layer speaks.
fn narrow(name: &[u16]) -> Vec<u8> {
    let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    String::from_utf16_lossy(&name[..end]).into_bytes()
}

pub(crate) fn mkdir(path: &[u8]) -> Result<(), Errno> {
    let path = wide_path(path)?;
    if unsafe { CreateDirectoryW(path.as_ptr(), core::ptr::null_mut()) } != 0 {
        Ok(())
    } else {
        Err(last_error())
    }
}

pub(crate) fn rmdir(path: &[u8]) -> Result<(), Errno> {
    let path = wide_path(path)?;
    if unsafe { RemoveDirectoryW(path.as_ptr()) } != 0 {
        Ok(())
    } else {
        Err(last_error())
    }
}

/// Replace `to` with `from`.
///
/// `MOVEFILE_REPLACE_EXISTING` is what makes this the same operation Unix's
/// `rename` is: without it Windows refuses when the destination exists, and an
/// atomic install would stop being atomic. It does *not* replace an existing
/// **directory** -- Windows has no equivalent for that -- which is why the
/// store publishes a directory by renaming into a name nothing holds yet.
pub(crate) fn rename(from: &[u8], to: &[u8]) -> Result<(), Errno> {
    let from = wide_path(from)?;
    let to = wide_path(to)?;
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_REPLACE_EXISTING) } != 0 {
        Ok(())
    } else {
        Err(last_error())
    }
}

fn attributes(path: &[u8]) -> Result<WIN32_FILE_ATTRIBUTE_DATA, Errno> {
    let path = wide_path(path)?;
    let mut data = WIN32_FILE_ATTRIBUTE_DATA {
        dwFileAttributes: 0,
        ftCreationTime: FILETIME { low: 0, high: 0 },
        ftLastAccessTime: FILETIME { low: 0, high: 0 },
        ftLastWriteTime: FILETIME { low: 0, high: 0 },
        nFileSizeHigh: 0,
        nFileSizeLow: 0,
    };
    let ok = unsafe {
        GetFileAttributesExW(
            path.as_ptr(),
            GetFileExInfoStandard,
            (&raw mut data).cast::<c_void>(),
        )
    };
    if ok == 0 { Err(last_error()) } else { Ok(data) }
}

pub(crate) fn is_dir(path: &[u8]) -> bool {
    match attributes(path) {
        Ok(data) => data.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0,
        Err(_) => false,
    }
}

pub(crate) fn file_size(path: &[u8]) -> Result<i64, Errno> {
    let data = attributes(path)?;
    Ok(((u64::from(data.nFileSizeHigh) << 32) | u64::from(data.nFileSizeLow)) as i64)
}

pub(crate) fn read_dir(path: &[u8]) -> Result<Vec<Vec<u8>>, Errno> {
    // `FindFirstFileW` takes a pattern rather than a directory, so the
    // wildcard is appended here. Win32 accepts `/` as a separator, which is
    // what lets everything above this layer use one spelling.
    let mut pattern = path.to_vec();
    while pattern.last() == Some(&b'/') || pattern.last() == Some(&b'\\') {
        pattern.pop();
    }
    pattern.extend_from_slice(b"/*");
    let pattern = wide_path(&pattern)?;

    let mut data = WIN32_FIND_DATAW {
        dwFileAttributes: 0,
        ftCreationTime: FILETIME { low: 0, high: 0 },
        ftLastAccessTime: FILETIME { low: 0, high: 0 },
        ftLastWriteTime: FILETIME { low: 0, high: 0 },
        nFileSizeHigh: 0,
        nFileSizeLow: 0,
        dwReserved0: 0,
        dwReserved1: 0,
        cFileName: [0; 260],
        cAlternateFileName: [0; 14],
    };
    let handle = unsafe { FindFirstFileW(pattern.as_ptr(), &raw mut data) };
    if handle == invalid_handle() {
        return Err(last_error());
    }
    let mut names = Vec::new();
    loop {
        let name = narrow(&data.cFileName);
        if name != b"." && name != b".." {
            names.push(name);
        }
        if unsafe { FindNextFileW(handle, &raw mut data) } == 0 {
            let e = last_error();
            unsafe { FindClose(handle) };
            // The end of the listing is reported as a failure with a code that
            // means "there were no more", which is not a failure at all.
            return if e.0 == ERROR_NO_MORE_FILES {
                Ok(names)
            } else {
                Err(e)
            };
        }
    }
}

pub(crate) fn env(name: &[u8]) -> Option<Vec<u8>> {
    let name = wide_path(name).ok()?;
    // Asked twice: the first call with no room answers with how much room it
    // wants, including the terminator, and zero means the variable is not set.
    let wanted = unsafe { GetEnvironmentVariableW(name.as_ptr(), core::ptr::null_mut(), 0) };
    if wanted == 0 {
        return None;
    }
    let mut buf = vec![0u16; wanted as usize];
    let written = unsafe { GetEnvironmentVariableW(name.as_ptr(), buf.as_mut_ptr(), wanted) };
    if written == 0 || written >= wanted {
        return None;
    }
    buf.truncate(written as usize);
    Some(narrow(&buf))
}

/// The working directory. `room` is ignored, and the answer is never `None`.
///
/// The two-call shape `GetEnvironmentVariableW` uses is available here too, and
/// it is exact: asking with no room answers with how much is wanted rather than
/// failing, so the growing loop [`super::cwd`] runs for the Unix arms never
/// goes round twice on this one.
///
/// The separator is left as Windows wrote it. `std/path.normalise` is what
/// turns a `\` that arrives from outside into a `/`, exactly as it does for
/// `env` -- this layer never invents one and never rewrites one.
pub(crate) fn cwd(_room: usize) -> Result<Option<Vec<u8>>, Errno> {
    let wanted = unsafe { GetCurrentDirectoryW(0, core::ptr::null_mut()) };
    if wanted == 0 {
        return Err(last_error());
    }
    let mut buf = vec![0u16; wanted as usize];
    let written = unsafe { GetCurrentDirectoryW(wanted, buf.as_mut_ptr()) };
    // `written` excludes the terminator that `wanted` counted, so a value that
    // reaches it means the directory changed underneath the two calls.
    if written == 0 || written >= wanted {
        return Err(last_error());
    }
    buf.truncate(written as usize);
    Ok(Some(narrow(&buf)))
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

/// The trust anchors, as a blob of length-prefixed DER certificates.
///
/// Windows' store is not a file path and not a directory: it is an API. The
/// "ROOT" system store is the one holding the anchors a browser would trust,
/// and `CertEnumCertificatesInStore` walks it, handing back a context whose
/// `pbCertEncoded` is the DER this library reads.
///
/// A third library, after kernel32 and ws2_32 and bcrypt -- the same shape the
/// generator needed.
pub(crate) fn system_roots() -> Option<Vec<u8>> {
    type HCERTSTORE = *mut c_void;

    /// Only the first three fields are read, but the whole layout has to be
    /// right for the offsets to be: `pbCertEncoded` is at word one.
    #[repr(C)]
    struct CertContext {
        encoding_type: DWORD,
        encoded: *const u8,
        encoded_len: DWORD,
        info: *mut c_void,
        store: HCERTSTORE,
    }

    #[link(name = "crypt32")]
    unsafe extern "system" {
        fn CertOpenSystemStoreW(provider: usize, subsystem: *const u16) -> HCERTSTORE;
        fn CertEnumCertificatesInStore(
            store: HCERTSTORE,
            previous: *const CertContext,
        ) -> *const CertContext;
        fn CertCloseStore(store: HCERTSTORE, flags: DWORD) -> BOOL;
    }

    // "ROOT", as UTF-16 with its terminator.
    let name: [u16; 5] = [b'R' as u16, b'O' as u16, b'O' as u16, b'T' as u16, 0];
    let store = unsafe { CertOpenSystemStoreW(0, name.as_ptr()) };
    if store.is_null() {
        return None;
    }
    let mut out = Vec::new();
    let mut context: *const CertContext = core::ptr::null();
    loop {
        // Each call frees the context it was given, so the previous pointer
        // must not be touched afterwards.
        context = unsafe { CertEnumCertificatesInStore(store, context) };
        if context.is_null() {
            break;
        }
        let der = unsafe {
            let c = &*context;
            if c.encoded.is_null() || c.encoded_len == 0 {
                continue;
            }
            core::slice::from_raw_parts(c.encoded, c.encoded_len as usize)
        };
        out.extend_from_slice(&(der.len() as u32).to_be_bytes());
        out.extend_from_slice(der);
    }
    unsafe { CertCloseStore(store, 0) };
    if out.is_empty() { None } else { Some(out) }
}
