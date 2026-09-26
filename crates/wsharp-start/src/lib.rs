// **Not compiled into a test harness, and this is where that has to be said.**
// This crate defines `main`; a test binary brings its own, and the linker
// refuses two entry symbols. `Cargo.toml` says so with `test = false`, and
// cargo honours that key only while the lib target is not *explicitly*
// selected -- passing a test-name filter (`cargo test --workspace arithmetic`)
// selects it, `test = false` stops applying, and the build fails with `entry
// symbol main declared multiple times`, naming neither the filter nor the
// manifest key. Reported as #8. The property belongs to the crate rather than
// to one of cargo's two routes to it, so it is stated here too; the manifest
// key stays, because it is still what keeps this crate out of the graph for
// the ordinary command. The `contributor commands (gate 6d)` job in
// `.github/workflows/release-gates.yml` is what notices if this stops holding:
// it is the only thing in CI that passes a filter, and a filter is the only
// way to the broken path.
#![cfg(not(test))]

//! What a compiled W# program starts and ends in.
//!
//! `wsharp build` emits an object file holding the program's code, its string
//! literals, and the three tables the collector needs. This is the other half:
//! the `main` a C runtime calls, which installs those tables, runs the
//! program's own `main`, and settles the runtime afterwards. It is the same
//! sequence `Jit::run` performs in the compiler, in the same order, and the
//! two are meant to be read side by side.
//!
//! It is a `staticlib` so that `cargo build` produces one `libwsharp_start.a`
//! with `wsharp-runtime` already inside it -- a compiled program then links
//! against one file, and `wsharp build` has one thing to find.
//!
//! # The four symbols this does not define
//!
//! [`ws_main`], [`ws_type_table`], [`ws_stack_maps`] and [`ws_service_table`]
//! are all in the object file `wsharp build` emits, so this archive is built
//! with them undefined and the final link is what resolves them. That is why
//! `cargo build -p wsharp-start` succeeds while linking this archive on its
//! own would not: an archive is allowed to want things.
//!
//! # Two things to know before changing this
//!
//! **Frame pointers.** The collector finds its roots by walking the
//! frame-pointer chain out of a runtime function, so every frame between it
//! and the nearest generated one must have a frame pointer. Generated code
//! preserves them because the code generator asks Cranelift to; this archive
//! does because `.cargo/config.toml` sets `-Cforce-frame-pointers=yes` for the
//! whole workspace. Build it any other way and the collector reads garbage on
//! the walk's first step.
//!
//! **`main` rather than Rust's.** Defining `main` here means Rust's
//! `lang_start` never runs. Everything this needs survives that -- panics,
//! unwinding, `RUST_BACKTRACE`, threads, and stdout, all of which initialise
//! lazily. Unix arguments arrive from the C runtime as bytes. On Windows,
//! `std::env::args_os` reads the wide command line even without `lang_start`,
//! avoiding the lossy code-page conversion of the C runtime's narrow `argv`.
//! The one thing genuinely lost is the main thread's
//! stack guard page, so a runaway recursion in W# is a segfault rather than a
//! message. That is what it already was under the JIT.

use core::ffi::{c_char, c_int};

unsafe extern "C" {
    /// The program's `main`, wrapped by the compiler so that a `void` one and
    /// an `i64` one are the same call. See `wsharp_codegen::ENTRY_SYMBOL`.
    fn ws_main() -> i64;

    /// The three tables, as `wsharp_codegen::tables` wrote them. Declared as
    /// bytes because their shape is `wsharp_runtime::aot`'s business, and only
    /// their addresses are wanted here.
    static ws_type_table: u8;
    static ws_stack_maps: u8;
    static ws_service_table: u8;
}

/// The entry point a C runtime calls.
///
/// # Safety
/// Called by the C runtime with the argument vector it was given.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn main(argc: c_int, argv: *const *const c_char) -> c_int {
    // Published before anything is compiled -- which here means before the
    // tables are installed and `ws_main` is called. `os::raw_args` reads
    // process-wide storage written once, in the same class as the type
    // registry and the stack maps.
    wsharp_runtime::os::set_args(unsafe { arguments(argc, argv) });

    // The collector cannot trace an object without a layout, and cannot find a
    // root without a stack map, so this must happen before any generated code
    // runs -- exactly where `compile_jit` registers them.
    unsafe {
        wsharp_runtime::aot::register_static_tables(
            &ws_type_table,
            &ws_stack_maps,
            &ws_service_table,
        )
    };

    let code = unsafe { ws_main() };

    // Every worker still parked on its queue would keep the process alive, and
    // one in the middle of a trace would be left half way through it.
    wsharp_runtime::process::close_all();
    wsharp_runtime::signals::restore();
    wsharp_runtime::rpc::stop_all();
    // Sockets the program left open. The kernel would close them anyway; doing
    // it here releases a listener's port before the next process wants it.
    wsharp_runtime::net::close_all();
    // A trace may still be in flight; settle it so the report is stable.
    wsharp_runtime::gc::quiesce();
    wsharp_runtime::gc::report_if_asked();

    // Same convention as a C program: the low byte of `main`'s result.
    (code & 0xff) as c_int
}

/// The program's own arguments, which is everything after its name.
///
/// `argv[0]` is dropped because `os.args()` has never included it: under
/// `wsharp run prog.ws a b` a program sees `a` and `b`, and a compiled one
/// must see the same two. What a program calls itself is `os.self_exe`'s
/// question, and it has a better answer than `argv[0]` anyway.
///
/// # Safety
/// `argv` must hold `argc` NUL-terminated strings.
#[cfg(not(windows))]
unsafe fn arguments(argc: c_int, argv: *const *const c_char) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    if argv.is_null() {
        return out;
    }
    for i in 1..argc.max(0) {
        let p = unsafe { *argv.add(i as usize) };
        if p.is_null() {
            continue;
        }
        // Bytes rather than a `str`: a path is whatever the system says it is,
        // and W# is handed the bytes it was given.
        let mut n = 0usize;
        while unsafe { *p.add(n) } != 0 {
            n += 1;
        }
        out.push(unsafe { core::slice::from_raw_parts(p as *const u8, n) }.to_vec());
    }
    out
}

/// Windows CRT argv uses the active code page, not UTF-8. Read the wide command
/// line through Rust's Windows argument parser, which also preserves quoting
/// and empty arguments. Unlike Unix's implementation it needs no argv capture
/// from Rust's main. Reject unpaired UTF-16 surrogates instead of substituting a
/// different path for the one supplied by the caller.
///
/// # Safety
/// Called at process startup; the narrow C arguments are deliberately unused.
#[cfg(windows)]
unsafe fn arguments(_argc: c_int, _argv: *const *const c_char) -> Vec<Vec<u8>> {
    std::env::args_os()
        .skip(1)
        .map(|arg| match arg.into_string() {
            Ok(value) => value.into_bytes(),
            Err(_) => {
                eprintln!("error: command-line argument is not valid Unicode");
                std::process::exit(1);
            }
        })
        .collect()
}
