//! Turning the object file `wsharp build` emits into an executable.
//!
//! Two things have to be found for that: the runtime archive, and a C
//! compiler to drive the link. Neither is in this binary, and both fail in
//! ways worth a sentence rather than an `errno`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The archive holding the runtime and the `main` a compiled program starts
/// in. Built from `crates/wsharp-start`.
const ARCHIVE: &str = "libwsharp_start.a";

/// Where to look for the runtime archive, in order.
///
/// The last of these is what makes a checkout work with no setup: `cargo
/// build` puts `wsharp` and `libwsharp_start.a` in the same directory. The
/// first is what makes an unusual installation work at all.
fn candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(explicit) = std::env::var_os("WSHARP_RUNTIME_LIB") {
        out.push(PathBuf::from(explicit));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        // A release tarball puts the archive under `lib/` beside the binary.
        out.push(dir.join("lib").join(ARCHIVE));
        out.push(dir.join("..").join("lib").join(ARCHIVE));
        // A cargo build puts it right there.
        out.push(dir.join(ARCHIVE));
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
        "cannot find {ARCHIVE}, which holds the W# runtime a compiled program \
         links against. Looked in:\n{}\nSet WSHARP_RUNTIME_LIB to its path.",
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
