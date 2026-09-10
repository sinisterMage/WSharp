//! The W# compiler, as a library.
//!
//! `main.rs` is the `wsharp` binary and is a few lines of clap over this; the
//! `ingot` binary is a few more. What both need is the same pipeline -- find
//! the files, check them, specialise, compile, run -- so it lives here rather
//! than in either.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::ValueEnum;
use wsharp_syntax::diag::{Diagnostic, Severity, SourceMap, render};

pub mod api;
pub mod link;
pub mod load;

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
pub enum Emit {
    /// The token stream.
    Tokens,
    /// The parsed syntax tree. A debugging aid: it prints whatever the syntax
    /// tree happens to hold, it is shared with the parser tests, and it carries
    /// no promise at all. `api` is the one to generate code from.
    Ast,
    /// The program's declared surface, resolved and versioned -- what a tool
    /// that generates W# reads. See `crate::api`.
    Api,
    /// The inferred signature of every top-level function.
    Types,
    /// The typed, monomorphised intermediate representation.
    Hir,
    /// The generated Cranelift IR. Compiles the program to get it, so this
    /// works under `check` as well as `run`.
    Clif,
    /// The relocatable object file, left unlinked. `build` only: it is the
    /// half of `build` that does not need a C compiler.
    Obj,
}

/// What to do once a program has been checked and specialised.
pub enum Action {
    /// Stop. `check`, and anything `--emit` answers on its own.
    Check,
    /// Compile into this process and run `main`.
    Run,
    /// Compile to a native executable.
    Build { out: PathBuf },
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
    action: Action,
) -> Result<ExitCode, String> {
    let run = !matches!(action, Action::Check);
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
    if emit == Some(Emit::Api) {
        // After type checking rather than beside `--emit=ast`, so that what is
        // printed always describes a program the compiler accepted: a generator
        // reading it never has to wonder whether what it is generating from
        // type-checks.
        let known: Vec<api::Module<'_>> = program
            .modules
            .iter()
            .map(|m| api::Module {
                path: &m.path,
                ast: &m.ast,
                specifiers: &m.imports,
            })
            .collect();
        print!("{}", api::emit(&known));
        return Ok(ExitCode::SUCCESS);
    }
    if emit == Some(Emit::Types) {
        for (name, ty) in &analysis.signatures {
            println!("{name}: {ty}");
        }
        return Ok(ExitCode::SUCCESS);
    }

    // ---- specialisation ----
    if analysis.program.entry.is_none() {
        // A library module has nothing to specialise and nothing to run.
        // Checking one is a thing to want; running one is not.
        if !run && emit.is_none() {
            return Ok(ExitCode::SUCCESS);
        }
        return Err(format!("{} has no `main` function", root.name()));
    }
    let mono = wsharp_sema::monomorphize(&analysis.program, &mut analysis.store);
    if report(map, &mono.diags) {
        return Ok(ExitCode::FAILURE);
    }
    // `check` stops here rather than above the specialisation stage, so that it
    // accepts exactly what `run` accepts. Monomorphisation is where a generic
    // call whose type variable nothing pinned gets reported -- `var b =
    // array.new(32);` with nothing to say what the elements are -- and stopping
    // before it made `check` the more permissive of the two. The diagnostic was
    // always right; which command gave it was not.
    if !run && emit.is_none() {
        return Ok(ExitCode::SUCCESS);
    }
    if emit == Some(Emit::Hir) {
        println!("{:#?}", mono.program);
        return Ok(ExitCode::SUCCESS);
    }

    // ---- code generation ----
    // `--emit=obj` stops half way through `build` and there is no half way
    // through the others. Said here rather than left to fall through every
    // branch below, which is what it did -- and `check --emit=obj` then reached
    // the JIT and *ran* the program.
    if emit == Some(Emit::Obj) && !matches!(action, Action::Build { .. }) {
        return Err("`--emit=obj` is `build`'s: there is no object file to stop at here".into());
    }
    let want_clif = emit == Some(Emit::Clif);
    if let Action::Build { out } = action {
        let object = wsharp_codegen::compile_object(&mono.program, &mut analysis.store, want_clif)
            .map_err(|e| e.to_string())?;
        if want_clif {
            print!("{}", object.clif);
            return Ok(ExitCode::SUCCESS);
        }
        return finish_build(&object.bytes, &out, emit == Some(Emit::Obj));
    }

    let options = wsharp_codegen::Options { gc_stress };
    let jit = wsharp_codegen::compile_jit(&mono.program, &mut analysis.store, &options, want_clif)
        .map_err(|e| e.to_string())?;
    if want_clif {
        print!("{}", jit.clif);
        return Ok(ExitCode::SUCCESS);
    }

    let status = jit.run();
    // Same convention as a C program: the low byte of `main`'s result.
    Ok(ExitCode::from((status & 0xff) as u8))
}

/// Write the object out, and link it unless only the object was asked for.
///
/// The object goes beside the executable rather than in a temporary directory,
/// so that a failed link leaves something to look at and `--emit=obj` and a
/// full build put the file in the same place.
fn finish_build(bytes: &[u8], out: &Path, object_only: bool) -> Result<ExitCode, String> {
    let object = if object_only {
        out.to_path_buf()
    } else {
        out.with_extension("o")
    };
    std::fs::write(&object, bytes)
        .map_err(|e| format!("cannot write {}: {e}", object.display()))?;
    if object_only {
        return Ok(ExitCode::SUCCESS);
    }
    link::link(&object, out)?;
    // The object has served its purpose. A failed link keeps it, above.
    let _ = std::fs::remove_file(&object);
    Ok(ExitCode::SUCCESS)
}

/// Print diagnostics; returns true if any of them were errors.
pub fn report(map: &SourceMap, diags: &[Diagnostic]) -> bool {
    for diag in diags {
        eprint!("{}", render(map, diag));
    }
    diags.iter().any(|d| d.severity == Severity::Error)
}
