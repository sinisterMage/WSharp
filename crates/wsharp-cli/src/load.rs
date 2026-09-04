//! Finding and reading the files a program is made of.
//!
//! A W# program is one root file plus everything it reaches through
//! `@import`. Resolving that is file-system work, so it lives here rather than
//! in the type checker, which is handed the finished set.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use wsharp_syntax::ast;
use wsharp_syntax::diag::{Diagnostic, SourceMap};

/// One parsed file, with the module path it is named by.
pub struct Loaded {
    pub path: String,
    pub ast: ast::Module,
    /// For each `@import` specifier this file writes, the module it resolves
    /// to. Two files may reach the same module by different relative paths --
    /// `./geometry.ws` from one directory is `./modules/geometry.ws` from
    /// another -- so what a specifier *means* is answered here, once, where
    /// the file system is.
    pub imports: HashMap<String, String>,
}

/// The result of following every import from a root file.
pub struct Program {
    /// The root module first, then everything it reached, in the order they
    /// were read.
    pub modules: Vec<Loaded>,
    pub map: SourceMap,
    pub diags: Vec<Diagnostic>,
}

/// Read `root` and everything it imports.
///
/// Errors that stop a file being read -- a missing file, a cycle -- are
/// returned as diagnostics rather than aborting, so a program with two broken
/// imports reports both.
pub fn load(root: &Path) -> Result<Program, String> {
    let mut loader = Loader {
        map: SourceMap::new(),
        modules: Vec::new(),
        diags: Vec::new(),
        files: HashMap::new(),
        loading: Vec::new(),
    };
    let text = std::fs::read_to_string(root)
        .map_err(|e| format!("cannot read {}: {e}", root.display()))?;
    loader.add(root, "main".into(), text);
    // The parts of the standard library written in W#. They are compiled with
    // the program like any other module -- monomorphised per element type,
    // and dropped entirely when nothing calls them.
    for (path, source) in wsharp_runtime::builtins::std_module_sources() {
        let base = loader.map.add(format!("<{path}>"), (*source).to_string());
        let (ast, mut diags) = wsharp_syntax::parse_at(source, base);
        loader.diags.append(&mut diags);
        // A standard-library module's own imports are standard-library paths,
        // which stand for themselves.
        let imports = source_imports(&ast)
            .into_iter()
            .map(|(spec, _)| (spec.clone(), spec))
            .collect();
        loader.modules.push(Loaded {
            path: (*path).to_string(),
            ast,
            imports,
        });
    }
    Ok(Program {
        modules: loader.modules,
        map: loader.map,
        diags: loader.diags,
    })
}

struct Loader {
    map: SourceMap,
    modules: Vec<Loaded>,
    diags: Vec<Diagnostic>,
    /// Canonical file path to the module path it was given, so a file reached
    /// twice is read once.
    files: HashMap<PathBuf, String>,
    /// The chain of files currently being read, for reporting a cycle.
    loading: Vec<PathBuf>,
}

impl Loader {
    fn add(&mut self, file: &Path, module_path: String, text: String) {
        let base = self.map.add(file.display().to_string(), text.clone());
        let (ast, mut diags) = wsharp_syntax::parse_at(&text, base);
        self.diags.append(&mut diags);

        let canonical = canonical(file);
        self.files.insert(canonical.clone(), module_path.clone());
        // Pushed before the imports are followed, so a file that reaches
        // itself is seen as a cycle rather than read again.
        self.loading.push(canonical);

        let specs = source_imports(&ast);
        // Reserve this module's slot before following its imports, so the root
        // module stays first and the order is the order files were reached.
        let slot = self.modules.len();
        self.modules.push(Loaded {
            path: module_path,
            ast,
            imports: HashMap::new(),
        });
        for (spec, span) in specs {
            if let Some(target) = self.follow(file, &spec, span) {
                self.modules[slot].imports.insert(spec, target);
            }
        }
        self.loading.pop();
    }

    /// Read one imported file and return the module path it is known by.
    ///
    /// `None` means the import failed and has been reported. A standard-library
    /// path resolves to itself: there is no file, and the type checker knows
    /// which ones exist.
    fn follow(&mut self, from: &Path, spec: &str, span: wsharp_syntax::Span) -> Option<String> {
        if spec == "std" || spec.starts_with("std/") {
            return Some(spec.to_string());
        }
        let dir = from.parent().unwrap_or_else(|| Path::new("."));
        let target = dir.join(spec);
        let resolved = canonical(&target);

        if self.loading.contains(&resolved) {
            let chain: Vec<String> = self
                .loading
                .iter()
                .map(|p| p.display().to_string())
                .collect();
            self.diags.push(
                Diagnostic::error(span, format!("`{spec}` imports itself, directly or not"))
                    .label("this import closes a cycle")
                    .help(format!("the chain is {}", chain.join(" -> "))),
            );
            return None;
        }
        // Reached twice by different relative paths: the same module.
        if let Some(path) = self.files.get(&resolved) {
            return Some(path.clone());
        }
        let text = match std::fs::read_to_string(&target) {
            Ok(text) => text,
            Err(e) => {
                self.diags.push(
                    Diagnostic::error(span, format!("cannot read `{spec}`: {e}"))
                        .help("an import path is relative to the file it is written in"),
                );
                return None;
            }
        };
        // A file's identity is where it is, not how it was spelled.
        let module_path = resolved.display().to_string();
        self.add(&target, module_path.clone(), text);
        Some(module_path)
    }
}

/// A path in a form two spellings of the same file agree on.
fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// The `@import` specifiers a parsed file names, in source order.
///
/// Only a top-level `const` may hold one, which is what makes the set of files
/// a program needs answerable before anything is type-checked.
fn source_imports(module: &ast::Module) -> Vec<(String, wsharp_syntax::Span)> {
    module
        .items
        .iter()
        .filter_map(|item| match item {
            ast::Item::Const(decl) => match &decl.value {
                ast::Expr::Import { path, span } => Some((path.to_string(), *span)),
                _ => None,
            },
            _ => None,
        })
        .collect()
}
