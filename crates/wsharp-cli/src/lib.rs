//! The W# compiler, as a library.
//!
//! `main.rs` is the `wsharp` binary and is a few lines of clap over this; the
//! `ingot` binary is a few more. What both need is the same pipeline -- find
//! the files, check them, specialise, compile, run -- so it lives here rather
//! than in either.

use std::path::Path;
use std::process::ExitCode;

use clap::ValueEnum;
use wsharp_syntax::diag::{Diagnostic, Severity, SourceMap, render};

pub mod load;

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
pub enum Emit {
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

/// What a program is rooted at: a file, or one of the library's own modules.
///
/// The second is how `ingot` runs. Its verbs are a W# program compiled into
/// this binary along with the rest of the library, so it has a module path and
/// no file name.
#[derive(Copy, Clone)]
pub enum Root<'a> {
    File(&'a Path),
    Module(&'a str),
}

impl Root<'_> {
    fn name(&self) -> String {
        match self {
            Root::File(path) => path.display().to_string(),
            Root::Module(module) => (*module).to_string(),
        }
    }
}

pub fn drive(
    root: Root<'_>,
    emit: Option<Emit>,
    gc_stress: bool,
    run: bool,
) -> Result<ExitCode, String> {
    // `--emit=tokens` is about one file, and has to work on one that does not
    // parse, so it does not go through the loader.
    if emit == Some(Emit::Tokens) {
        let Root::File(path) = root else {
            return Err("`--emit=tokens` wants a file".into());
        };
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
    let program = match root {
        Root::File(path) => load::load(path)?,
        Root::Module(module) => load::load_library_module(module)?,
    };
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
        return Err(format!("{} has no `main` function", root.name()));
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
pub fn report(map: &SourceMap, diags: &[Diagnostic]) -> bool {
    for diag in diags {
        eprint!("{}", render(map, diag));
    }
    diags.iter().any(|d| d.severity == Severity::Error)
}
