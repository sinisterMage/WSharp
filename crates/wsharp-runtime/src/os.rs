//! The process's own arguments and environment.
//!
//! The command line is the first thing a program written in W# needs that W#
//! could not say: `main` takes no arguments, by the type checker's own rule,
//! and `Jit::run` passes it the one word every W# function takes, which is the
//! closure environment pointer rather than an `argc`. So the arguments arrive
//! the way the type registry and the stack maps do -- published once into
//! process-wide storage before any generated code runs, and read from there.
//!
//! That is the third thing in this runtime allowed to be process-wide, and it
//! qualifies for the same reason the other two do: it is frozen before the
//! program starts and never written again.

use crate::io::FallibleStr;
use crate::strings::{alloc_str, str_bytes};
use crate::sys;
use std::sync::OnceLock;

/// The arguments this program was given, without the executable's own name.
///
/// Set by whichever driver compiled the program -- `wsharp run` after its own
/// flags, `ingot` from its whole command line -- and never after that.
static ARGS: OnceLock<Vec<Vec<u8>>> = OnceLock::new();

/// Publish the command line. Called once, before the program runs.
///
/// A second call is ignored rather than reported: there is one program per
/// process and nothing sensible to do with a second answer.
pub fn set_args(args: Vec<Vec<u8>>) {
    let _ = ARGS.set(args);
}

/// `?str`, as it crosses the boundary: the tag in a whole word, then the value.
///
/// Zero is null and one is a value, which is what [`crate::repr`]'s option tag
/// means -- the same shape the broker's `raw_poll` answers with.
#[repr(C)]
pub struct MaybeStr {
    pub tag: i64,
    pub value: *mut u8,
}

/// The command line, as a blob of length-prefixed arguments.
///
/// A `str` for the reason `fs.raw_read_dir` is one, and the same four-byte
/// big-endian framing: a builtin may not allocate an array, so `std/os.args`
/// cuts the blob up in W#.
///
/// Infallible. A program always has a command line, and one with no arguments
/// has an empty one, which is an empty blob rather than an error.
#[unsafe(no_mangle)]
pub extern "C" fn ws_os_raw_args() -> *mut u8 {
    unsafe { crate::gc::checkpoint() };
    let empty: Vec<Vec<u8>> = Vec::new();
    let args = ARGS.get().unwrap_or(&empty);
    alloc_str(&crate::fs::length_prefixed(args))
}

/// What this build was built for, as a target triple.
///
/// `x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`, and so on -- spelled the
/// way the compiler that produced this binary spelled it, because the string is
/// set from cargo's own `TARGET` by `build.rs` rather than reassembled here.
/// [`std::env::consts`] cannot answer this: it has `ARCH` and `OS` and no
/// vendor or environment between them.
///
/// The question a program asks when it has to fetch something built for the
/// machine it is running on, which is the whole of a version manager's first
/// step. Infallible -- a build always has a target -- and constant, so there is
/// nothing to block on and no `worker::blocking` here.
///
/// The AOT and JIT answers agree: a compiled program links the runtime archive
/// built for its own target, and `wsharp run` executes this out of a `wsharp`
/// built for the machine it is running on.
#[unsafe(no_mangle)]
pub extern "C" fn ws_os_target() -> *mut u8 {
    unsafe { crate::gc::checkpoint() };
    alloc_str(env!("WSHARP_TARGET").as_bytes())
}

/// One environment variable, or null.
///
/// Not an error union: a variable that is not set is the ordinary case, and
/// `orelse` is what a caller wants to write. `HOME` on Unix and `USERPROFILE`
/// on Windows are what a store's location is worked out from, and asking for
/// the wrong one of those is not a failure either.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; `name` must be null or
/// point at a W# string object, and `out` must point at storage laid out as a
/// [`MaybeStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_os_env(out: *mut MaybeStr, name: *const u8) {
    unsafe { crate::gc::checkpoint() };
    let name = unsafe { str_bytes(name) }.to_vec();
    // Reading the environment is a memory access rather than a syscall, but it
    // is the platform's memory: the safe region costs nothing and keeps every
    // call through `sys` under one rule.
    let found = crate::worker::blocking(|| sys::env(&name));
    let result = match found {
        Some(value) => MaybeStr {
            tag: 1,
            value: alloc_str(&value),
        },
        None => MaybeStr {
            tag: 0,
            value: std::ptr::null_mut(),
        },
    };
    unsafe { out.write(result) };
}

/// The process's working directory.
///
/// Fallible where `home` and `temp_dir` are not, and for a reason worth stating:
/// those two ask the environment, which either says something or does not, while
/// this asks the kernel about a directory that can have been removed since the
/// process entered it. A program that has lost its working directory is in a
/// situation `orelse` cannot describe.
///
/// Answers with what the system said, separators and all. `std/path.normalise`
/// is what turns a Windows `\` into a `/`; `std/os` does not import `std/path`,
/// because a path is arithmetic and the environment is a fact about the process.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; `out` must point at
/// storage laid out as a [`FallibleStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_os_cwd(out: *mut FallibleStr) {
    unsafe { crate::gc::checkpoint() };
    let result = match crate::worker::blocking(sys::cwd) {
        Ok(dir) => FallibleStr::ok(alloc_str(&dir)),
        Err(e) => FallibleStr::err(sys::error_tag(e)),
    };
    unsafe { out.write(result) };
}

/// Change the process's working directory.
///
/// The only writable piece of process-wide state this runtime offers, and it
/// exists for one caller: a tool told to work somewhere else, as `git -C` is.
/// Such a tool does it once, before any verb runs. Doing it half way through a
/// program would make every relative path in it depend on when it was reached,
/// which is why W# has no `defer`-style scoping for this and should not.
///
/// # Safety
/// Called from generated code across an FFI boundary; `dir` must be null or
/// point at a W# string object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_os_chdir(dir: *const u8) -> i64 {
    unsafe { crate::gc::checkpoint() };
    // Copied into plain bytes before the safe region, because a reference held
    // in a Rust local is described by no stack map and the collector may run
    // while this thread is parked.
    let dir = unsafe { str_bytes(dir) }.to_vec();
    match crate::worker::blocking(|| sys::chdir(&dir)) {
        Ok(()) => 0,
        Err(e) => sys::error_tag(e),
    }
}

/// End the process now, with `code` as its status.
///
/// The off switch a program that starts workers needs. `main` returning ends
/// the process too, but it goes through `rpc::stop_all` first, which waits for
/// every worker that can be stopped -- and a program that wants out from
/// somewhere else, or from inside a worker, has nothing else to say so.
///
/// It does *not* stop the workers, deliberately: an exit that can be blocked by
/// a worker refusing to stop is not an exit. What it does do is what every
/// other way out of this runtime does -- release the sockets, so a listener's
/// port is free before the next process wants it, and print the collector's
/// statistics if they were asked for, so `WSHARP_GC_STATS=1` says the same
/// thing however the program ended.
///
/// The low byte, as C has it and as `main`'s return value already is.
///
/// # Safety
/// Called from generated code across an FFI boundary.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_os_exit(code: i64) -> ! {
    crate::net::close_all();
    crate::gc::report_if_asked();
    std::process::exit((code & 0xff) as i32);
}

/// The path of the running executable.
/// The path of the running executable.
///
/// What a program needs to find something installed beside it -- which is how
/// `ingot` finds `wsharp`. Not `argv[0]`: that is whatever the caller passed
/// to `exec`, and a program found through `PATH` gets a bare name back.
///
/// # Safety
/// Called from generated code across an FFI boundary; `out` must point at
/// storage laid out as a [`FallibleStr`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_os_self_exe(out: *mut FallibleStr) {
    unsafe { crate::gc::checkpoint() };
    let result = match crate::worker::blocking(sys::self_exe) {
        Ok(path) => FallibleStr::ok(alloc_str(&path)),
        Err(e) => FallibleStr::err(sys::error_tag(e)),
    };
    unsafe { out.write(result) };
}

/// Replace this process with another program.
///
/// Answers only on failure: there is no caller left to answer on success. That
/// is why the return is `!void` and not `!i64` -- a status would be a promise
/// this cannot keep.
///
/// `argv` arrives as one blob of four-byte big-endian lengths and their bytes,
/// the same framing [`ws_os_raw_args`] answers with -- and for a reason that is
/// the mirror of that one. A `[]str` holds *references*, and every reference
/// generated code loads goes through the load barrier; a Rust function reaching
/// into the array would be reading them by another route, with nothing to
/// resolve one the collector has already moved. So `std/os.exec` packs the
/// array in W#, where the barrier applies by construction, and this reads
/// bytes -- which is all a builtin may ever do.
///
/// The arguments are what the new program sees *without* its own name: the
/// name is `program`, and this writes it in front itself. So it lines up with
/// `os.args()`, which has never included the name, rather than with the C
/// convention it would otherwise inherit.
///
/// # Safety
/// Called from generated code across an FFI boundary; `program` and `argv`
/// must be null or point at W# string objects.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_os_exec(program: *const u8, argv: *const u8) -> i64 {
    unsafe { crate::gc::checkpoint() };
    let program = unsafe { str_bytes(program) }.to_vec();
    let blob = unsafe { str_bytes(argv) }.to_vec();
    // The name the new program sees as its own comes first, which is what a C
    // `argv` means and what every shell does.
    let mut owned: Vec<Vec<u8>> = vec![program.clone()];
    owned.extend(unpack(&blob));
    // Nothing on the heap is touched from here: every byte `exec` needs is
    // already copied, which is what a safe region requires of its caller.
    let e = crate::worker::blocking(|| sys::exec(&program, &owned));
    sys::error_tag(e)
}

/// The inverse of [`crate::fs::length_prefixed`].
///
/// A length the blob cannot hold ends the walk rather than being reported: the
/// blob was written by `std/os.pack` beside this file, so a malformed one is a
/// bug here and not something a caller could act on. The same rule
/// `std/os.unpack` states for the other direction.
fn unpack(blob: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + 4 <= blob.len() {
        let size = u32::from_be_bytes(blob[at..at + 4].try_into().expect("four bytes")) as usize;
        at += 4;
        if at + size > blob.len() {
            return out;
        }
        out.push(blob[at..at + size].to_vec());
        at += size;
    }
    out
}
