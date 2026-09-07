//! `ingot`, the W# package manager.
//!
//! Almost none of it is here. The verbs are `ingot/main.ws`, compiled into this
//! binary along with the rest of the library, and this file publishes the
//! command line and runs them. That is the point rather than an economy: a
//! resolver, a hash, a protocol and a file format is a broad enough program to
//! find out what W# is actually missing, and every gap it finds is one a user
//! would have found instead.
//!
//! Two things are the driver's own, and both for the same reason -- they *are*
//! the compiler, which is what this binary embeds and what a W# program has no
//! way to ask for:
//!
//!   * `ingot run <file.ws> [args]`, which compiles a program and runs it.
//!   * `--gc-stress`, which belongs to whatever is being run.
//!   * `-C <dir>`, which is a property of the process rather than of a verb.
//!
//! Everything else is handed to `ingot/main`, which reads it from `os.args()`.
//! It is a seam rather than a split: the verb list a user sees is one list, and
//! it is printed by the W# half.
//!
//! `wsharp` stays the compiler and this stays the tool, exactly as the roadmap
//! asked. One of them has to work on a machine with no network and no store,
//! and the other is the thing that fills the store.

use std::path::PathBuf;
use std::process::ExitCode;

use wsharp_cli::{Root, drive};

/// The module `ingot`'s verbs live in.
const VERBS: &str = wsharp_runtime::builtins::INGOT_MAIN_MODULE;

/// What a verb that could not be carried out at all exits with. The same
/// number `ingot/main.ws` uses, and separate from `verify`'s answers, which are
/// about a project rather than about a mistake.
const FAILED: u8 = 4;

fn main() -> ExitCode {
    // Not clap: the arguments belong to the W# half, which reads them itself,
    // and a parser here would have to be kept in step with the one there.
    let mut args: Vec<String> = std::env::args().skip(1).collect();

    // `--gc-stress` is the compiler's rather than a verb's, so it is taken out
    // wherever it appears and never reaches `os.args()`.
    let gc_stress = args.iter().any(|a| a == "--gc-stress");
    args.retain(|a| a != "--gc-stress");

    // `-C <dir>` runs as if from somewhere else, as `git -C` does. The driver's
    // rather than a verb's because W# has no `chdir` and should not: a process
    // has one working directory, and changing it half way through a program
    // would be a thing every path in every verb had to know about.
    if args.first().map(String::as_str) == Some("-C") {
        if args.len() < 2 {
            eprintln!("ingot: `-C` needs a directory");
            return ExitCode::from(FAILED);
        }
        let dir = args.remove(1);
        args.remove(0);
        if let Err(e) = std::env::set_current_dir(&dir) {
            eprintln!("ingot: cannot work in {dir}: {e}");
            return ExitCode::from(FAILED);
        }
    }

    let result = match args.first().map(String::as_str) {
        Some("run") => run_program(&args[1..], gc_stress),
        _ => {
            wsharp_runtime::os::set_args(args.into_iter().map(String::into_bytes).collect());
            drive(Root::Module(VERBS), None, gc_stress, true)
        }
    };
    match result {
        Ok(code) => code,
        Err(message) => {
            eprintln!("ingot: {message}");
            ExitCode::from(FAILED)
        }
    }
}

/// `ingot run <file.ws> [args]`.
///
/// The one verb that is the compiler. Everything after the file is the
/// program's, exactly as it is under `wsharp run`.
fn run_program(args: &[String], gc_stress: bool) -> Result<ExitCode, String> {
    let Some(file) = args.first() else {
        return Err("`run` needs a file".into());
    };
    let rest = args[1..]
        .iter()
        .skip_while(|a| *a == "--")
        .cloned()
        .map(String::into_bytes)
        .collect();
    wsharp_runtime::os::set_args(rest);
    drive(Root::File(&PathBuf::from(file)), None, gc_stress, true)
}
