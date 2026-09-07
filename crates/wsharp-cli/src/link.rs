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
