//! The `wsharp` command line driver.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use wsharp_syntax::diag::{Diagnostic, Severity, SourceMap, render};

mod load;

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
    },
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
enum Emit {
    /// The token stream.
    Tokens,
    /// The parsed syntax tree.
    Ast,
    /// The inferred signature of every top-level function.
    Types,
    /// The typed, monomorphised intermediate representation.
    Hir,
    /// The generated Cranelift IR. Compiles the program to get it, so this
    /// works under `check` as well as `run`.
    Clif,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let (path, emit, gc_stress, run) = match cli.command {
        Command::Check { file, emit } => (file, emit, false, false),
        Command::Run {
            file,
            emit,
            gc_stress,
        } => (file, emit, gc_stress, true),
    };

    match drive(&path, emit, gc_stress, run) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn drive(
    path: &PathBuf,
    emit: Option<Emit>,
    gc_stress: bool,
    run: bool,
) -> Result<ExitCode, String> {
    // `--emit=tokens` is about one file, and has to work on one that does not
    // parse, so it does not go through the loader.
    if emit == Some(Emit::Tokens) {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let mut map = SourceMap::new();
        let base = map.add(path.display().to_string(), text.clone());
        let (tokens, lex_diags) = wsharp_syntax::lexer::lex_at(&text, base);
        // Tokens first: they are what was asked for, and the lexer recovers
        // from every error, so the stream is complete even when it has some.
        for token in &tokens {
            println!("{:>12?}  {:?}", token.span, token.kind);
        }
        if report(&map, &lex_diags) {
            return Ok(ExitCode::FAILURE);
        }
        return Ok(ExitCode::SUCCESS);
    }

    // ---- syntax ----
    let program = load::load(path)?;
    let map = &program.map;
    if report(map, &program.diags) {
        return Ok(ExitCode::FAILURE);
    }
    if emit == Some(Emit::Ast) {
        for module in &program.modules {
            if program.modules.len() > 1 {
                println!("; {}", module.path);
            }
            print!("{}", wsharp_syntax::dump::dump_module(&module.ast));
        }
        return Ok(ExitCode::SUCCESS);
    }

    // ---- types ----
    let modules: Vec<wsharp_sema::SourceModule> = program
        .modules
        .iter()
        .map(|m| wsharp_sema::SourceModule {
            path: m.path.clone(),
            ast: &m.ast,
            imports: m.imports.clone(),
        })
        .collect();
    let mut analysis = wsharp_sema::analyze_program(&modules);
    if report(map, &analysis.diags) {
        return Ok(ExitCode::FAILURE);
    }
    if emit == Some(Emit::Types) {
        for (name, ty) in &analysis.signatures {
            println!("{name}: {ty}");
        }
        return Ok(ExitCode::SUCCESS);
    }

    if !run && emit.is_none() {
        return Ok(ExitCode::SUCCESS);
    }

    // ---- specialisation ----
    if analysis.program.entry.is_none() {
        return Err(format!("{} has no `main` function", path.display()));
    }
    let mono = wsharp_sema::monomorphize(&analysis.program, &mut analysis.store);
    if report(map, &mono.diags) {
        return Ok(ExitCode::FAILURE);
    }
    if emit == Some(Emit::Hir) {
        println!("{:#?}", mono.program);
        return Ok(ExitCode::SUCCESS);
    }

    // ---- code generation ----
    let options = wsharp_codegen::Options { gc_stress };
    let want_clif = emit == Some(Emit::Clif);
    let jit = wsharp_codegen::compile(&mono.program, &mut analysis.store, &options, want_clif)
        .map_err(|e| e.to_string())?;
    if want_clif {
        print!("{}", jit.clif);
        return Ok(ExitCode::SUCCESS);
    }

    let status = jit.run();
    // Same convention as a C program: the low byte of `main`'s result.
    Ok(ExitCode::from((status & 0xff) as u8))
}

/// Print diagnostics; returns true if any of them were errors.
fn report(map: &SourceMap, diags: &[Diagnostic]) -> bool {
    for diag in diags {
        eprint!("{}", render(map, diag));
    }
    diags.iter().any(|d| d.severity == Severity::Error)
}
