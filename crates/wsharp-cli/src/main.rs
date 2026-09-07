//! The `wsharp` command line driver.
//!
//! Everything below the flags is in the library beside this file, because the
//! `ingot` binary needs the same pipeline.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use wsharp_cli::{Emit, Root, drive};

/// Printed after the option list, since the collector's switches are
/// environment variables rather than flags: they are read by the runtime,
/// which a compiled program reaches without going through this driver.
const AFTER_HELP: &str = "\
Environment:
  WSHARP_GC_STATS=1  print collector statistics on exit
  WSHARP_GC_TRACE=1  print every frame the root walk visits";

#[derive(Parser)]
#[command(
    name = "wsharp",
    version,
    about = "The W# compiler",
    after_help = AFTER_HELP
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Type-check a program without running it.
    Check {
        file: PathBuf,
        /// Print an intermediate form instead of just checking.
        #[arg(long, value_enum)]
        emit: Option<Emit>,
    },
    /// Compile a program and run its `main`.
    ///
    /// The process exits with `main`'s return value, as a C program would.
    Run {
        file: PathBuf,
        #[arg(long, value_enum)]
        emit: Option<Emit>,
        /// Collect at every allocation and check every root found.
        ///
        /// Turns a missed garbage collection root from a rare corruption into
        /// an immediate, reproducible failure. Ruinously slow; for testing.
        #[arg(long)]
        gc_stress: bool,
        /// What the program sees as `os.args()`.
        ///
        /// Everything after the file, or after `--` when an argument would
        /// otherwise be read as one of this driver's own flags. Nothing is
        /// interpreted on the way through.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let (path, emit, gc_stress, run, args) = match cli.command {
        Command::Check { file, emit } => (file, emit, false, false, Vec::new()),
        Command::Run {
            file,
            emit,
            gc_stress,
            args,
        } => (file, emit, gc_stress, true, args),
    };
    // Published before anything is compiled, let alone run: `os.raw_args`
    // reads process-wide storage that is written once, in the same class as
    // the type registry and the stack maps.
    //
    // A `--` that clap left in front is dropped: it is this driver's
    // punctuation, not the program's first argument.
    let mut args: Vec<String> = args;
    if args.first().map(String::as_str) == Some("--") {
        args.remove(0);
    }
    wsharp_runtime::os::set_args(args.into_iter().map(String::into_bytes).collect());

    match drive(Root::File(&path), emit, gc_stress, run) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}
