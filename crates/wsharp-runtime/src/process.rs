//! Child ownership and bounded capture. No W# heap pointer survives blocking.
//!
//! Rust's Command supplies argv/environment encoding and safe spawn in a
//! multithreaded process. Pipe readiness and signals live in platform.rs. Both
//! streams are pumped fairly without reader threads; a grandchild holding a
//! pipe cannot strand a thread after timeout or close.
use crate::builtins::{
    Builtin, BuiltinTy as B, ERROR_BAD_FORMAT, ERROR_IO_FAILED, ERROR_NOT_SUPPORTED,
    ERROR_OUTPUT_LIMIT, ERROR_TIMED_OUT,
};
use crate::io::{FallibleI64, FallibleStr};
use crate::strings::{alloc_str, str_bytes};
use std::collections::HashMap;
use std::io::{self, Read};
use std::process::{Child, ChildStderr, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

mod platform;

struct Process {
    child: Child,
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
    out: Vec<u8>,
    err: Vec<u8>,
    limit: usize,
    status: Option<ExitStatus>,
    fault: Option<i64>,
    group: bool,
    closed: bool,
}
#[derive(Default)]
struct Registry {
    next: i64,
    children: HashMap<i64, Arc<Mutex<Process>>>,
    closing: bool,
}
fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
}
fn error(e: io::Error) -> i64 {
    match e.kind() {
        io::ErrorKind::NotFound => crate::builtins::ERROR_NOT_FOUND,
        io::ErrorKind::PermissionDenied => crate::builtins::ERROR_PERMISSION_DENIED,
        io::ErrorKind::InvalidInput => ERROR_BAD_FORMAT,
        io::ErrorKind::Unsupported => ERROR_NOT_SUPPORTED,
        _ => ERROR_IO_FAILED,
    }
}
fn unpack(blob: &[u8]) -> Result<Vec<Vec<u8>>, i64> {
    let mut rest = blob;
    let mut result = Vec::new();
    while !rest.is_empty() {
        if rest.len() < 4 {
            return Err(ERROR_BAD_FORMAT);
        }
        let len = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
        rest = &rest[4..];
        if len > rest.len() {
            return Err(ERROR_BAD_FORMAT);
        }
        result.push(rest[..len].to_vec());
        rest = &rest[len..];
    }
    Ok(result)
}
fn os_string(bytes: Vec<u8>) -> Result<std::ffi::OsString, i64> {
    if bytes.contains(&0) {
        return Err(ERROR_BAD_FORMAT);
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(std::ffi::OsString::from_vec(bytes))
    }
    #[cfg(not(unix))]
    {
        String::from_utf8(bytes)
            .map(Into::into)
            .map_err(|_| ERROR_BAD_FORMAT)
    }
}
fn stdio(mode: i64, input: bool) -> Result<Stdio, i64> {
    match mode {
        0 => Ok(Stdio::null()),
        1 => Ok(Stdio::inherit()),
        2 if !input => Ok(Stdio::piped()),
        _ => Err(ERROR_BAD_FORMAT),
    }
}
struct Spec {
    program: Vec<u8>,
    args: Vec<Vec<u8>>,
    cwd: Vec<u8>,
    env: Vec<Vec<u8>>,
    clear_env: bool,
    stdin: i64,
    stdout: i64,
    stderr: i64,
    limit: i64,
    group: bool,
}
fn spawn(spec: Spec) -> Result<i64, i64> {
    // Validate completely before starting anything.
    if spec.program.is_empty()
        || spec.limit < 0
        || spec.limit > 1024 * 1024 * 1024
        || !spec.env.len().is_multiple_of(2)
    {
        return Err(ERROR_BAD_FORMAT);
    }
    let mut cmd = Command::new(os_string(spec.program)?);
    for arg in spec.args {
        cmd.arg(os_string(arg)?);
    }
    if !spec.cwd.is_empty() {
        cmd.current_dir(os_string(spec.cwd)?);
    }
    if spec.clear_env {
        cmd.env_clear();
    }
    for pair in spec.env.chunks_exact(2) {
        if pair[0].is_empty() || pair[0].contains(&b'=') {
            return Err(ERROR_BAD_FORMAT);
        }
        cmd.env(os_string(pair[0].clone())?, os_string(pair[1].clone())?);
    }
    cmd.stdin(stdio(spec.stdin, true)?)
        .stdout(stdio(spec.stdout, false)?)
        .stderr(stdio(spec.stderr, false)?);
    platform::configure(&mut cmd, spec.group).map_err(error)?;
    // Serialize publication with close_all so shutdown cannot miss a new child.
    let mut table = registry().lock().unwrap_or_else(|e| e.into_inner());
    if table.closing {
        return Err(ERROR_IO_FAILED);
    }
    let child = cmd.spawn().map_err(error)?;
    let mut p = Process {
        child,
        stdout: None,
        stderr: None,
        out: Vec::new(),
        err: Vec::new(),
        limit: spec.limit as usize,
        status: None,
        fault: None,
        group: spec.group,
        closed: false,
    };
    p.stdout = p.child.stdout.take();
    p.stderr = p.child.stderr.take();
    if let Err(e) = platform::prepare(p.stdout.as_ref(), p.stderr.as_ref()) {
        p.close();
        return Err(error(e));
    }
    table.next = table.next.checked_add(1).ok_or(ERROR_IO_FAILED)?;
    let id = table.next;
    table.children.insert(id, Arc::new(Mutex::new(p)));
    Ok(id)
}
fn get(id: i64) -> Result<Arc<Mutex<Process>>, i64> {
    registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .children
        .get(&id)
        .cloned()
        .ok_or(ERROR_IO_FAILED)
}

// At most 64 KiB per stream per turn: an endlessly writing stdout cannot starve
// stderr, status checks, another worker's kill, or the monotonic deadline.
fn drain<R: Read + platform::Pipe>(
    pipe: &mut Option<R>,
    output: &mut Vec<u8>,
    room: &mut usize,
) -> Result<(), i64> {
    let Some(reader) = pipe.as_mut() else {
        return Ok(());
    };
    let mut buf = [0u8; 8192];
    for _ in 0..8 {
        let size = buf.len().min(room.saturating_add(1));
        match platform::read(reader, &mut buf[..size]) {
            Ok(0) => {
                *pipe = None;
                break;
            }
            Ok(n) => {
                if n > *room {
                    return Err(ERROR_OUTPUT_LIMIT);
                }
                output.extend_from_slice(&buf[..n]);
                *room -= n;
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(ERROR_IO_FAILED),
        }
    }
    Ok(())
}
impl Process {
    fn pump(&mut self) -> Result<(), i64> {
        if self.closed {
            return Err(ERROR_IO_FAILED);
        }
        if let Some(fault) = self.fault {
            return Err(fault);
        }
        let mut room = self.limit.saturating_sub(self.out.len() + self.err.len());
        let result = drain(&mut self.stdout, &mut self.out, &mut room)
            .and_then(|()| drain(&mut self.stderr, &mut self.err, &mut room));
        if let Err(e) = result {
            self.fault = Some(e);
            let _ = platform::signal(&mut self.child, self.group, 9);
            self.stdout = None;
            self.stderr = None;
            return Err(e);
        }
        if self.status.is_none() {
            self.status = self.child.try_wait().map_err(|_| ERROR_IO_FAILED)?;
        }
        Ok(())
    }
    fn packed(&self, output: bool) -> Vec<u8> {
        let status = self.status.expect("exit was checked");
        let (code, signal) = platform::status(status);
        let mut fields = vec![
            code.to_string().into_bytes(),
            signal.to_string().into_bytes(),
        ];
        if output {
            fields.push(self.out.clone());
            fields.push(self.err.clone());
        }
        crate::fs::length_prefixed(&fields)
    }
    fn close(&mut self) {
        if self.closed {
            return;
        }
        // Signal before reaping: an unreaped child's PID cannot be reused.
        // If poll already reaped it, never signal that number again.
        if self.status.is_none() {
            let _ = platform::signal(&mut self.child, self.group, 9);
            self.status = self.child.wait().ok();
        }
        self.stdout = None;
        self.stderr = None;
        self.out.clear();
        self.err.clear();
        self.closed = true;
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        self.close();
    }
}
fn close(id: i64) {
    let p = registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .children
        .remove(&id);
    if let Some(p) = p {
        p.lock().unwrap_or_else(|e| e.into_inner()).close();
    }
}
/// Normal end-of-program cleanup. Explicit exit uses kill_all instead.
pub fn close_all() {
    crate::worker::blocking(|| {
        let children = {
            let mut table = registry().lock().unwrap_or_else(|e| e.into_inner());
            table.closing = true;
            std::mem::take(&mut table.children)
        };
        for p in children.into_values() {
            p.lock().unwrap_or_else(|e| e.into_inner()).close();
        }
    });
}
/// Best-effort kill on forced exit. Never wait on a child or a runtime lock:
/// os.exit must still be usable as the final bound on a stuck operation. The
/// exiting parent's remaining children are reparented and reaped by the OS.
pub fn kill_all() {
    let Ok(mut table) = registry().try_lock() else {
        return;
    };
    table.closing = true;
    for child in table.children.values() {
        if let Ok(mut p) = child.try_lock()
            && p.status.is_none()
            && !p.closed
        {
            let group = p.group;
            let _ = platform::signal(&mut p.child, group, 9);
        }
    }
}
fn wait(id: i64, timeout: i64, output: bool) -> Result<Vec<u8>, i64> {
    let p = get(id)?;
    let start = Instant::now();
    loop {
        {
            let mut p = p.lock().unwrap_or_else(|e| e.into_inner());
            p.pump()?;
            if p.status.is_some() && (!output || (p.stdout.is_none() && p.stderr.is_none())) {
                return Ok(p.packed(output));
            }
        }
        if timeout >= 0 && start.elapsed() >= Duration::from_millis(timeout as u64) {
            return Err(ERROR_TIMED_OUT);
        }
        let delay = if timeout < 0 {
            Duration::from_millis(2)
        } else {
            Duration::from_millis(2)
                .min(Duration::from_millis(timeout as u64).saturating_sub(start.elapsed()))
        };
        std::thread::sleep(delay);
    }
}

/// # Safety
/// String pointers name W# strings; out is return storage. Copy before blocking.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_process_spawn(
    out: *mut FallibleI64,
    program: *const u8,
    args: *const u8,
    cwd: *const u8,
    env: *const u8,
    clear_env: i8,
    stdin: i64,
    stdout: i64,
    stderr: i64,
    limit: i64,
    group: i8,
) {
    unsafe { crate::gc::checkpoint() };
    let owned = unsafe {
        (
            str_bytes(program).to_vec(),
            str_bytes(args).to_vec(),
            str_bytes(cwd).to_vec(),
            str_bytes(env).to_vec(),
        )
    };
    let result = crate::worker::blocking(|| {
        spawn(Spec {
            program: owned.0,
            args: unpack(&owned.1)?,
            cwd: owned.2,
            env: unpack(&owned.3)?,
            clear_env: clear_env != 0,
            stdin,
            stdout,
            stderr,
            limit,
            group: group != 0,
        })
    });
    unsafe {
        out.write(match result {
            Ok(id) => FallibleI64::ok(id),
            Err(e) => FallibleI64::err(e),
        })
    };
}
fn string_result(result: Result<Vec<u8>, i64>) -> FallibleStr {
    match result {
        Ok(bytes) => FallibleStr::ok(alloc_str(&bytes)),
        Err(e) => FallibleStr::err(e),
    }
}
/// # Safety
/// out is return storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_process_poll(out: *mut FallibleStr, id: i64) {
    unsafe { crate::gc::checkpoint() };
    let result = crate::worker::blocking(|| {
        let p = get(id)?;
        let mut p = p.lock().unwrap_or_else(|e| e.into_inner());
        p.pump()?;
        Ok(if p.status.is_some() {
            p.packed(false)
        } else {
            Vec::new()
        })
    });
    unsafe { out.write(string_result(result)) };
}
/// # Safety
/// out is return storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_process_wait(out: *mut FallibleStr, id: i64, timeout: i64, output: i8) {
    unsafe { crate::gc::checkpoint() };
    let result = crate::worker::blocking(|| wait(id, timeout, output != 0));
    unsafe { out.write(string_result(result)) };
}
#[unsafe(no_mangle)]
pub extern "C" fn ws_process_signal(id: i64, signal: i64) -> i64 {
    unsafe { crate::gc::checkpoint() };
    crate::worker::blocking(|| {
        let p = match get(id) {
            Ok(p) => p,
            Err(e) => return e,
        };
        let mut p = p.lock().unwrap_or_else(|e| e.into_inner());
        if p.closed {
            return ERROR_IO_FAILED;
        }
        if p.status.is_some() {
            return 0;
        }
        if ![2, 9, 15].contains(&signal) {
            return ERROR_NOT_SUPPORTED;
        }
        let group = p.group;
        match platform::signal(&mut p.child, group, signal as i32) {
            Ok(()) => 0,
            Err(e) if e.kind() == io::ErrorKind::Unsupported => ERROR_NOT_SUPPORTED,
            Err(_) => ERROR_IO_FAILED,
        }
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn ws_process_close(id: i64) {
    unsafe { crate::gc::checkpoint() };
    crate::worker::blocking(|| close(id));
}

pub(crate) fn builtins() -> Vec<Builtin> {
    vec![
        Builtin {
            module: "std/process",
            name: "raw_spawn",
            params: &[
                B::Str,
                B::Str,
                B::Str,
                B::Str,
                B::Bool,
                B::I64,
                B::I64,
                B::I64,
                B::I64,
                B::Bool,
            ],
            ret: B::ErrUnion(
                &B::I64,
                &[
                    "NotFound",
                    "PermissionDenied",
                    "IoFailed",
                    "BadFormat",
                    "NotSupported",
                ],
            ),
            link: "ws_process_spawn",
            ptr: ws_process_spawn as *const u8,
        },
        Builtin {
            module: "std/process",
            name: "raw_poll",
            params: &[B::I64],
            ret: B::ErrUnion(&B::Str, &["IoFailed", "OutputLimit"]),
            link: "ws_process_poll",
            ptr: ws_process_poll as *const u8,
        },
        Builtin {
            module: "std/process",
            name: "raw_wait",
            params: &[B::I64, B::I64, B::Bool],
            ret: B::ErrUnion(&B::Str, &["IoFailed", "OutputLimit", "TimedOut"]),
            link: "ws_process_wait",
            ptr: ws_process_wait as *const u8,
        },
        Builtin {
            module: "std/process",
            name: "raw_signal",
            params: &[B::I64, B::I64],
            ret: B::ErrUnion(&B::Void, &["IoFailed", "NotSupported"]),
            link: "ws_process_signal",
            ptr: ws_process_signal as *const u8,
        },
        Builtin {
            module: "std/process",
            name: "raw_close",
            params: &[B::I64],
            ret: B::Void,
            link: "ws_process_close",
            ptr: ws_process_close as *const u8,
        },
    ]
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    fn shell(script: &str, limit: i64) -> i64 {
        spawn(Spec {
            program: b"/bin/sh".to_vec(),
            args: vec![b"-c".to_vec(), script.as_bytes().to_vec()],
            cwd: Vec::new(),
            env: Vec::new(),
            clear_env: false,
            stdin: 0,
            stdout: 2,
            stderr: 2,
            limit,
            group: true,
        })
        .unwrap()
    }
    #[test]
    fn framing_rejects_truncated_and_trailing_bytes() {
        assert_eq!(unpack(&[0, 0, 0]), Err(ERROR_BAD_FORMAT));
        assert_eq!(unpack(&[0, 0, 0, 5, 1]), Err(ERROR_BAD_FORMAT));
        assert_eq!(unpack(&[0, 0, 0, 0, 1]), Err(ERROR_BAD_FORMAT));
        assert_eq!(unpack(&[0, 0, 0, 0]).unwrap(), vec![Vec::<u8>::new()]);
    }
    #[test]
    fn nul_in_arguments_is_rejected_before_spawn() {
        assert_eq!(
            spawn(Spec {
                program: b"/bin/sh".to_vec(),
                args: vec![b"x\0y".to_vec()],
                cwd: Vec::new(),
                env: Vec::new(),
                clear_env: false,
                stdin: 0,
                stdout: 2,
                stderr: 2,
                limit: 10,
                group: false
            }),
            Err(ERROR_BAD_FORMAT)
        );
    }
    #[test]
    fn capture_exact_limit_and_nonzero_status() {
        let id = shell("printf abc; printf def >&2; exit 23", 6);
        let result = unpack(&wait(id, 2000, true).unwrap()).unwrap();
        close(id);
        assert_eq!(
            result,
            vec![
                b"23".to_vec(),
                b"0".to_vec(),
                b"abc".to_vec(),
                b"def".to_vec()
            ]
        );
    }
    #[test]
    fn zero_limit_distinguishes_empty_output_from_overflow() {
        let quiet = shell("exit 0", 0);
        assert!(wait(quiet, 2000, true).is_ok());
        close(quiet);
        let noisy = shell("printf x", 0);
        assert_eq!(wait(noisy, 2000, true), Err(ERROR_OUTPUT_LIMIT));
        close(noisy);
    }
    #[test]
    fn output_limit_is_combined_across_streams() {
        let id = shell("printf abc; printf def >&2", 5);
        assert_eq!(wait(id, 2000, true), Err(ERROR_OUTPUT_LIMIT));
        close(id);
    }
    #[test]
    fn timeout_can_be_followed_by_another_wait() {
        let id = shell("sleep 0.04; printf done", 10);
        assert_eq!(wait(id, 0, true), Err(ERROR_TIMED_OUT));
        let out = unpack(&wait(id, 2000, true).unwrap()).unwrap();
        close(id);
        assert_eq!(out[2], b"done");
    }
    #[test]
    fn waiting_never_holds_the_registry_or_child_lock_while_sleeping() {
        let id = shell("sleep 30", 10);
        let waiter = std::thread::spawn(move || wait(id, 5000, true));
        std::thread::sleep(Duration::from_millis(10));
        let start = Instant::now();
        close(id);
        assert!(start.elapsed() < Duration::from_secs(2));
        assert_eq!(waiter.join().unwrap(), Err(ERROR_IO_FAILED));
    }
    #[test]
    fn a_descendant_retaining_a_pipe_does_not_remove_the_capture_deadline() {
        let id = shell("sleep 0.2 & exit 0", 10);
        let result = wait(id, 50, true);
        assert_eq!(result, Err(ERROR_TIMED_OUT));
        // Direct child is reaped, but collect must still wait for pipe EOF.
        let out = unpack(&wait(id, 2000, true).unwrap()).unwrap();
        close(id);
        assert_eq!(out[0], b"0");
    }
    #[test]
    fn closing_a_handle_never_retargets_a_later_spawn() {
        let a = shell("sleep 30", 0);
        close(a);
        let b = shell("exit 0", 0);
        assert_ne!(a, b);
        close(a);
        assert!(wait(b, 2000, true).is_ok());
        close(b);
        assert!(get(a).is_err());
        assert!(get(b).is_err());
    }
}
