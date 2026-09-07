//! Finding and reading the files a program is made of.
//!
//! A W# program is one root file plus everything it reaches through
//! `@import`. Resolving that is file-system work, so it lives here rather than
//! in the type checker, which is handed the finished set.

use std::collections::{HashMap, HashSet};
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

/// The tables `@import` resolves a library path against.
fn libraries() -> [&'static [(&'static str, &'static str)]; 2] {
    [
        wsharp_runtime::builtins::std_module_sources(),
        wsharp_runtime::builtins::ingot_module_sources(),
    ]
}

/// Read `root` and everything it imports.
///
/// Errors that stop a file being read -- a missing file, a cycle -- are
/// returned as diagnostics rather than aborting, so a program with two broken
/// imports reports both.
pub fn load(root: &Path) -> Result<Program, String> {
    let mut loader = Loader::new();
    let text = std::fs::read_to_string(root)
        .map_err(|e| format!("cannot read {}: {e}", root.display()))?;
    loader.add(root, "main".into(), text);
    loader.add_library(&libraries());
    Ok(loader.finish())
}

/// The same, for a program whose root is a library module rather than a file.
///
/// `ingot` is a W# program that is compiled into the binary along with the rest
/// of the library, so there is no file to name. Everything else is identical --
/// the module's own imports are library paths, its name is its module path, and
/// what it reaches is loaded exactly as it would be for a program on disk.
pub fn load_library_module(root: &str) -> Result<Program, String> {
    let source = libraries()
        .iter()
        .flat_map(|table| table.iter())
        .find(|(path, _)| *path == root)
        .map(|(_, source)| *source)
        .ok_or_else(|| format!("there is no library module `{root}`"))?;

    let mut loader = Loader::new();
    let base = loader.map.add(format!("<{root}>"), source.to_string());
    let (ast, mut diags) = wsharp_syntax::parse_at(source, base);
    loader.diags.append(&mut diags);
    let imports = source_imports(&ast)
        .into_iter()
        .map(|(spec, _)| (spec.clone(), spec))
        .collect();
    loader.modules.push(Loaded {
        path: root.to_string(),
        ast,
        imports,
    });
    loader.add_library(&libraries());
    Ok(loader.finish())
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

/// Whether an import specifier names a library module rather than a file
/// beside the one that wrote it.
///
/// Two namespaces: `std`, and `ingot` for the package manager's own modules.
/// A third rule -- a package, resolved through a lockfile -- is item 11's
/// stage five, and goes in `Loader::follow` beside this one.
fn is_library(spec: &str) -> bool {
    wsharp_runtime::builtins::LIBRARY_ROOTS.iter().any(|root| {
        spec == *root
            || spec
                .strip_prefix(root)
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

/// The namespace a library specifier is in, for `@import("std")`.
fn library_root(spec: &str) -> Option<&'static str> {
    wsharp_runtime::builtins::LIBRARY_ROOTS
        .iter()
        .copied()
        .find(|root| spec == *root)
}

impl Loader {
    fn new() -> Loader {
        Loader {
            map: SourceMap::new(),
            modules: Vec::new(),
            diags: Vec::new(),
            files: HashMap::new(),
            loading: Vec::new(),
        }
    }

    fn finish(self) -> Program {
        Program {
            modules: self.modules,
            map: self.map,
            diags: self.diags,
        }
    }

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

    /// Add the parts of the standard library written in W# that this program
    /// can actually reach.
    ///
    /// They are compiled with the program like any other module --
    /// monomorphised per element type, and dropped entirely when nothing calls
    /// them -- but they still have to be *parsed and inferred*, and that is
    /// not free. Every module went in unconditionally while the library was
    /// four files, which stopped being reasonable at twenty-odd: a ten-line
    /// program spent more time inferring TLS and TOML than anything else.
    ///
    /// So a library module is read only when something imports it, following
    /// each one's own imports in turn. `@import("std")` means all of them,
    /// because a module path is a prefix and any of them can be walked into
    /// from there.
    ///
    /// The order is the table's, so what a program is built from does not
    /// depend on the order it happened to mention things in.
    fn add_library(&mut self, tables: &[&'static [(&'static str, &'static str)]]) {
        let sources: Vec<(&'static str, &'static str)> =
            tables.iter().flat_map(|t| t.iter().copied()).collect();
        let table: HashMap<&str, &str> = sources.iter().copied().collect();

        let mut wanted: HashSet<&str> = HashSet::new();
        let mut roots: HashSet<&'static str> = HashSet::new();
        for module in &self.modules {
            for spec in module.imports.values() {
                if let Some(root) = library_root(spec) {
                    roots.insert(root);
                } else if let Some((path, _)) = table.get_key_value(spec.as_str()) {
                    wanted.insert(path);
                }
            }
        }
        // A bare namespace is every module in it, because a module path is a
        // prefix and any of them can be walked into from there.
        for root in roots {
            let prefix = format!("{root}/");
            wanted.extend(table.keys().copied().filter(|p| p.starts_with(&prefix)));
        }

        // Parse each wanted module once, and follow what it imports. A library
        // module's imports are library paths, which stand for themselves.
        let mut parsed: HashMap<&str, (ast::Module, HashMap<String, String>)> = HashMap::new();
        let mut queue: Vec<&str> = wanted.iter().copied().collect();
        // A root that is itself a library module -- which is what `ingot` runs
        // -- is already here, and reading it again would declare everything in
        // it twice.
        let already: HashSet<&str> = self.modules.iter().map(|m| m.path.as_str()).collect();
        while let Some(path) = queue.pop() {
            if parsed.contains_key(path) || already.contains(path) {
                continue;
            }
            let Some(source) = table.get(path) else {
                continue;
            };
            let base = self.map.add(format!("<{path}>"), (*source).to_string());
            let (module, mut diags) = wsharp_syntax::parse_at(source, base);
            self.diags.append(&mut diags);
            let mut imports = HashMap::new();
            for (spec, _) in source_imports(&module) {
                if is_library(&spec)
                    && let Some((next, _)) = table.get_key_value(spec.as_str())
                {
                    queue.push(next);
                }
                imports.insert(spec.clone(), spec);
            }
            parsed.insert(path, (module, imports));
        }

        for (path, _) in &sources {
            if let Some((ast, imports)) = parsed.remove(*path) {
                self.modules.push(Loaded {
                    path: (*path).to_string(),
                    ast,
                    imports,
                });
            }
        }
    }

    /// Read one imported file and return the module path it is known by.
    ///
    /// `None` means the import failed and has been reported. A standard-library
    /// path resolves to itself: there is no file, and the type checker knows
    /// which ones exist.
    fn follow(&mut self, from: &Path, spec: &str, span: wsharp_syntax::Span) -> Option<String> {
        if is_library(spec) {
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
