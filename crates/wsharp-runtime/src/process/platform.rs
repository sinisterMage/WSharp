//! The only native details Command does not provide: nonblocking pipe reads,
//! process-group signals, and console/Unix termination notification.
use std::io;
use std::process::{Child, ChildStderr, ChildStdout, Command, ExitStatus};

#[cfg(unix)]
mod imp {
    use super::*;
    use std::os::fd::AsRawFd;
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    unsafe extern "C" {
        fn fcntl(fd: i32, command: i32, ...) -> i32;
        fn kill(pid: i32, signal: i32) -> i32;
    }
    pub trait Pipe: AsRawFd + io::Read {}
    impl<T: AsRawFd + io::Read> Pipe for T {}
    pub fn configure(cmd: &mut Command, group: bool) -> io::Result<()> {
        if group {
            cmd.process_group(0);
        }
        Ok(())
    }
    fn nonblocking(p: &impl AsRawFd) -> io::Result<()> {
        #[cfg(target_os = "linux")]
        const NONBLOCK: i32 = 0o4000;
        #[cfg(not(target_os = "linux"))]
        const NONBLOCK: i32 = 4;
        let fd = p.as_raw_fd();
        let flags = unsafe { fcntl(fd, 3) }; // F_GETFL
        if flags < 0 || unsafe { fcntl(fd, 4, flags | NONBLOCK) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    pub fn prepare(out: Option<&ChildStdout>, err: Option<&ChildStderr>) -> io::Result<()> {
        if let Some(p) = out {
            nonblocking(p)?;
        }
        if let Some(p) = err {
            nonblocking(p)?;
        }
        Ok(())
    }
    pub fn read(p: &mut impl Pipe, buf: &mut [u8]) -> io::Result<usize> {
        p.read(buf)
    }
    pub fn signal(child: &mut Child, group: bool, signal: i32) -> io::Result<()> {
        let pid = child.id() as i32;
        if unsafe { kill(if group { -pid } else { pid }, signal) } < 0 {
            let e = io::Error::last_os_error();
            if e.raw_os_error() != Some(3) {
                return Err(e);
            } // ESRCH: already gone
        }
        Ok(())
    }
    pub fn status(s: ExitStatus) -> (i64, i64) {
        (
            s.code().map(i64::from).unwrap_or(-1),
            i64::from(s.signal().unwrap_or(0)),
        )
    }
}
#[cfg(windows)]
mod imp {
    use super::*;
    use std::os::windows::io::AsRawHandle;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn PeekNamedPipe(
            pipe: *mut std::ffi::c_void,
            buffer: *mut u8,
            size: u32,
            read: *mut u32,
            available: *mut u32,
            left: *mut u32,
        ) -> i32;
    }
    pub trait Pipe: AsRawHandle + io::Read {}
    impl<T: AsRawHandle + io::Read> Pipe for T {}
    pub fn configure(_cmd: &mut Command, group: bool) -> io::Result<()> {
        if group {
            return Err(io::ErrorKind::Unsupported.into());
        }
        Ok(())
    }
    pub fn prepare(_out: Option<&ChildStdout>, _err: Option<&ChildStderr>) -> io::Result<()> {
        Ok(())
    }
    pub fn read(p: &mut impl Pipe, buf: &mut [u8]) -> io::Result<usize> {
        let mut available = 0;
        let ok = unsafe {
            PeekNamedPipe(
                p.as_raw_handle(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut available,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            let e = io::Error::last_os_error();
            if e.raw_os_error() == Some(109) {
                return Ok(0);
            } // ERROR_BROKEN_PIPE
            return Err(e);
        }
        if available == 0 {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let n = buf.len().min(available as usize);
        p.read(&mut buf[..n])
    }
    pub fn signal(child: &mut Child, _group: bool, signal: i32) -> io::Result<()> {
        if signal != 9 {
            return Err(io::ErrorKind::Unsupported.into());
        }
        child.kill()
    }
    pub fn status(s: ExitStatus) -> (i64, i64) {
        (s.code().map(i64::from).unwrap_or(-1), 0)
    }
}
pub(super) use imp::*;
