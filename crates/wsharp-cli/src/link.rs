//! Turning the object file `wsharp build` emits into an executable.
//!
//! Two things have to be found for that: the runtime archive, and a C
//! compiler to drive the link. Neither is in this binary, and both fail in
//! ways worth a sentence rather than an `errno`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// What the archive holding the runtime is called, which is not one name.
///
/// `crates/wsharp-start` is a `staticlib`, and cargo names one after the
/// platform's own convention rather than after the crate: the Unix spelling is
/// `libwsharp_start.a`, and MSVC's is `wsharp_start.lib`. A `windows-gnu`
/// build keeps the Unix spelling, so the platform alone does not settle it.
///
/// Both are looked for on Windows rather than one being chosen from
/// `target_env`, because the question is what is *on disk* next to this binary
/// -- and a `wsharp` can perfectly well be handed an archive built by the other
/// toolchain. Asking is cheaper than deciding, and it cannot be wrong.
const ARCHIVE_NAMES: &[&str] = if cfg!(target_os = "windows") {
    &["wsharp_start.lib", "libwsharp_start.a"]
} else {
    &["libwsharp_start.a"]
};

/// Where to look for the runtime archive, in order.
///
/// The last of these is what makes a checkout work with no setup: `cargo
/// build` puts `wsharp` and the archive in the same directory. The first is
/// what makes an unusual installation work at all.
///
/// Each directory is tried under every name the archive can have, directory by
/// directory rather than name by name: an installation that somehow holds both
/// spellings should use the one nearest this binary, not the one that happens
/// to be listed first.
fn candidates() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut out = Vec::new();
    if let Some(explicit) = std::env::var_os("WSHARP_RUNTIME_LIB") {
        // Names a file, not a directory, so it is taken as given -- including
        // its name, which is the point of being able to set it.
        out.push(PathBuf::from(explicit));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        // A release tarball puts the archive under `lib/` beside the binary.
        dirs.push(dir.join("lib"));
        dirs.push(dir.join("..").join("lib"));
        // A cargo build puts it right there.
        dirs.push(dir.to_path_buf());
    }
    for dir in dirs {
        for name in ARCHIVE_NAMES {
            out.push(dir.join(name));
        }
    }
    out
}

fn find_archive() -> Result<PathBuf, String> {
    let tried = candidates();
    for path in &tried {
        if path.is_file() {
            return Ok(path.clone());
        }
    }
    let list: Vec<String> = tried.iter().map(|p| format!("  {}", p.display())).collect();
    Err(format!(
        "cannot find {}, which holds the W# runtime a compiled program \
         links against. Looked in:\n{}\nSet WSHARP_RUNTIME_LIB to its path.",
        ARCHIVE_NAMES.join(" or "),
        list.join("\n")
    ))
}

/// Link `object` into an executable at `out`.
pub fn link(object: &Path, out: &Path) -> Result<(), String> {
    let archive = find_archive()?;
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".to_string());

    let mut command = Command::new(&cc);
    command.arg(object).arg(&archive).arg("-o").arg(out);
    // What a Rust staticlib wants underneath it. Modern glibc folds the first
    // three into libc and ignores them; naming them keeps older systems and
    // the BSDs working.
    if !cfg!(target_os = "windows") {
        command.args(["-lpthread", "-lm", "-ldl"]);
    }
    // Windows needs to be told this is a console program, and the reason is
    // exact: **clang chooses the subsystem by looking for `main` in the object
    // files it was handed**, and ours is not in one. `main` is defined in
    // `wsharp-start`, which reaches the link as a *library* -- so clang sees an
    // object with no entry point, passes no `-subsystem:` at all, and the MSVC
    // linker has nothing to infer an entry from:
    //
    //     link.exe -out:prog.exe -defaultlib:libcmt -nologo prog.o wsharp_start.lib
    //     LINK : fatal error LNK1561: entry point must be defined
    //
    // Saying `console` restores the default entry, `mainCRTStartup`, which
    // references `main` and so pulls it out of the archive. Nothing equivalent
    // is needed on Unix, where `ld` resolves `main` from an archive for
    // `crt1.o` without being asked.
    if cfg!(target_os = "windows") {
        command.args(["-Xlinker", "-subsystem:console"]);
        // **The same C runtime the archive was built against.** Rust's MSVC
        // target links the *dynamic* CRT by default; clang's driver defaults to
        // the *static* one and passes `-defaultlib:libcmt`. Handing a Rust
        // staticlib to a static CRT is not a near miss -- the two disagree
        // about which allocator owns the heap, and the first symptom is a
        // symbol that only the dynamic form defines:
        //
        //     error LNK2019: unresolved external symbol __imp__wspawnvp
        //
        // The `__imp_` prefix is the tell: it is a *DLL import*, which is what
        // `msvcrt.lib` provides and `libcmt.lib` does not.
        command.arg("-fms-runtime-lib=dll");
        // And the system libraries underneath, for the same reason the two
        // frameworks are named below: **a staticlib does not carry what it
        // depends on.** `#[link(name = "ws2_32")]` in `sys/windows.rs` tells
        // *rustc* what to link when rustc is doing the linking, and travels no
        // further; a `.lib` handed to `clang` is a bag of objects with
        // undefined symbols in it. Without these the entry point resolves and
        // the sockets do not:
        //
        //     LINK : fatal error LNK1120: unresolved externals
        //
        // The first five are what rustc itself passes for any Windows program
        // -- they are what `std` needs -- and the last three are this
        // runtime's own: `bcrypt` for `crypto.random`, `crypt32` for the root
        // store, `advapi32` beside them. A library nothing references costs
        // nothing, so the list errs towards complete.
        command.args([
            "-lkernel32",
            "-lntdll",
            "-luserenv",
            "-lws2_32",
            "-ldbghelp",
            "-lbcrypt",
            "-lcrypt32",
            "-ladvapi32",
        ]);
    }
    // Apple's arms read the platform root store through these two
    // (`sys::bsd::system_roots`), and a staticlib carries no linker directives
    // here the way an MSVC one does. cargo passes them when *it* links
    // `wsharp`, because `#[link(kind = "framework")]` travels in the rlib --
    // so the JIT works and only a built program fails, with an undefined
    // `_SecTrustCopyAnchorCertificates` at the far end of a link. Naming them
    // is the whole fix.
    if cfg!(target_os = "macos") || cfg!(target_os = "ios") {
        command.args(["-framework", "Security", "-framework", "CoreFoundation"]);
    }

    let output = command.output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            // The trap this project opens its notes with: a bare NixOS box has
            // no `cc` on PATH, and the failure is otherwise a bare `os error 2`
            // from a program the user never asked to run.
            format!(
                "`wsharp build` needs a C compiler to link, and found no `{cc}` \
                 on PATH. Set $CC to one, or run inside `nix-shell`."
            )
        } else {
            format!("could not run `{cc}`: {e}")
        }
    })?;

    if !output.status.success() {
        return Err(format!(
            "linking failed:\n{}",
            String::from_utf8_lossy(&output.stderr).trim_end()
        ));
    }
    Ok(())
}
