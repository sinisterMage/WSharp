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
    // Found from the root *file* rather than from the process's directory,
    // because `wsharp run app/src/main.ws` has no `-C` and need not be run
    // inside the project it is compiling.
    loader.packages = Packages::found_from(root.parent().unwrap_or(Path::new(".")));
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
    /// What `ingot install` left beside the project's manifest. Empty for a
    /// program that is not in a project, which is most of them.
    packages: Packages,
}

/// One package this project has installed, as `ingot.env` records it.
#[derive(Debug, PartialEq, Eq)]
struct Package {
    name: String,
    /// Absolute: the project's own directory for the root package, and a store
    /// entry for everything else.
    dir: PathBuf,
    /// The facade, relative to `dir`. One file per package -- what a package of
    /// several presents through it is what `pub const x = other.x;` is for.
    root: PathBuf,
    /// What this package's own manifest asked for, and so what it may import.
    deps: Vec<String>,
}

/// The packages a program can name, and why it can name none.
///
/// The root package is first, which is what makes it the fallback owner: a file
/// that is in no store entry is the project's own.
#[derive(Default)]
struct Packages {
    entries: Vec<Package>,
    /// An `ingot.toml` was found and no readable `ingot.env` beside it. The
    /// project exists and has not been installed, which is a different thing to
    /// tell somebody than "no such package".
    uninstalled: bool,
}

impl Packages {
    /// Walk up from `dir` looking for a project, and read its environment.
    ///
    /// A program with nothing to do with ingot pays one directory walk for
    /// this and nothing else: with no `ingot.toml` above it there is no table,
    /// and `follow` never looks at one.
    fn found_from(dir: &Path) -> Packages {
        let mut at = canonical(dir);
        loop {
            if at.join("ingot.toml").is_file() {
                let text = std::fs::read_to_string(at.join("ingot.env"));
                return match text.ok().and_then(|t| parse_env(&t, &at)) {
                    Some(entries) => Packages {
                        entries,
                        uninstalled: false,
                    },
                    // Unreadable and malformed are the same answer, because
                    // the file is derived: whatever is wrong with it, writing
                    // it again is the fix.
                    None => Packages {
                        entries: Vec::new(),
                        uninstalled: true,
                    },
                };
            }
            if !at.pop() {
                return Packages::default();
            }
        }
    }

    fn find(&self, spec: &str) -> Option<usize> {
        self.entries.iter().position(|p| p.name == spec)
    }

    /// Which package a file belongs to.
    ///
    /// The entry whose directory is the **longest** prefix of it, and the root
    /// package when none is. Longest rather than first because `WSHARP_HOME`
    /// may sit inside the project -- which is exactly what `ingot`'s own verb
    /// tests do -- so a store entry can be under the project's directory too.
    /// A file under neither is one the project reached by a relative path of
    /// its own, and that makes it the project's.
    fn owner_of(&self, file: &Path) -> Option<&Package> {
        let mut best: Option<&Package> = None;
        for entry in &self.entries {
            if file.starts_with(&entry.dir)
                && best.is_none_or(|b| entry.dir.as_os_str().len() > b.dir.as_os_str().len())
            {
                best = Some(entry);
            }
        }
        best.or_else(|| self.entries.first())
    }
}

/// Read `ingot.env`: one line per package, tab-separated, the root first.
///
/// `name`, its directory, its facade relative to that, and then one field per
/// dependency. `None` for a line that does not hold at least the first three,
/// because the file is generated and a shape this does not know is a file from
/// another version rather than something to guess at.
///
/// A relative directory is taken against the project, which nothing writes
/// today and costs one line to be right about.
fn parse_env(text: &str, project: &Path) -> Option<Vec<Package>> {
    let mut entries = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let mut fields = line.split('\t');
        let name = fields.next()?;
        let dir = fields.next()?;
        let root = fields.next()?;
        if name.is_empty() || dir.is_empty() || root.is_empty() {
            return None;
        }
        entries.push(Package {
            name: name.to_string(),
            dir: canonical(&project.join(dir)),
            root: PathBuf::from(root),
            deps: fields.map(str::to_string).collect(),
        });
    }
    Some(entries)
}

/// Whether a specifier is *spelled* the way a package name is.
///
/// Only a hint, and only used to decide which help to print: what a specifier
/// means is answered by the tables, never by its shape. Every relative import
/// in this project writes `./` and ends in `.ws`.
fn looks_like_a_package(spec: &str) -> bool {
    !spec.starts_with('.') && !spec.ends_with(".ws")
}

/// Whether an import specifier names a library module rather than a file
/// beside the one that wrote it.
///
/// Two namespaces: `std`, and `ingot` for the package manager's own modules.
/// A package is the third rule and is *not* a namespace: it is a name in this
/// project's `ingot.env`, so `std` and `ingot` stay reserved by being asked
/// about first.
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
            packages: Packages::default(),
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
    /// `None` means the import failed and has been reported. Three rules, in
    /// this order:
    ///
    ///   1. A standard-library path resolves to itself: there is no file, and
    ///      the type checker knows which ones exist.
    ///   2. A package this project has installed resolves to its facade.
    ///   3. Anything else is a file beside the one that wrote it.
    ///
    /// The library first is what keeps `std` and `ingot` from being names a
    /// package can take.
    fn follow(&mut self, from: &Path, spec: &str, span: wsharp_syntax::Span) -> Option<String> {
        if is_library(spec) {
            return Some(spec.to_string());
        }
        if let Some(index) = self.packages.find(spec) {
            return self.follow_package(from, index, spec, span);
        }
        let dir = from.parent().unwrap_or_else(|| Path::new("."));
        let target = dir.join(spec);
        self.follow_file(&target, spec, span, self.missing_file_help(spec))
    }

    /// A package: its facade, and only if the importing package asked for it.
    ///
    /// The scope check is the point. `ingot.env` describes the whole graph,
    /// because the solver chooses one version of a package for the whole
    /// project -- so without it every package could reach every other package
    /// anything in the project happens to depend on, and a manifest would
    /// describe what gets fetched rather than what may be named.
    fn follow_package(
        &mut self,
        from: &Path,
        index: usize,
        spec: &str,
        span: wsharp_syntax::Span,
    ) -> Option<String> {
        if let Some(owner) = self.packages.owner_of(from)
            && !owner.deps.iter().any(|d| d == spec)
        {
            let name = owner.name.clone();
            self.diags.push(
                Diagnostic::error(span, format!("`{spec}` is not a dependency of `{name}`"))
                    .label("this package was never asked for")
                    .help(format!(
                        "a package may import what its own `ingot.toml` names; \
                         `ingot add {spec}` records one"
                    )),
            );
            return None;
        }
        let entry = &self.packages.entries[index];
        let target = entry.dir.join(&entry.root);
        let help = format!(
            "`{spec}` is installed at {}, and that is the file its `ingot.toml` names -- \
             `ingot verify` says whether the entry is intact",
            entry.dir.display()
        );
        self.follow_file(&target, spec, span, help)
    }

    /// Read `target`, or report why not, and answer with its module path.
    ///
    /// Shared by the last two rules, because everything after "which file is
    /// it" is the same question: has this been read, is it a cycle, and can it
    /// be opened. Only the help differs, and it is what says which rule failed.
    fn follow_file(
        &mut self,
        target: &Path,
        spec: &str,
        span: wsharp_syntax::Span,
        help: String,
    ) -> Option<String> {
        let resolved = canonical(target);

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
        let text = match std::fs::read_to_string(target) {
            Ok(text) => text,
            Err(e) => {
                self.diags
                    .push(Diagnostic::error(span, format!("cannot read `{spec}`: {e}")).help(help));
                return None;
            }
        };
        // A file's identity is where it is, not how it was spelled.
        let module_path = module_path_of(&resolved);
        self.add(target, module_path.clone(), text);
        Some(module_path)
    }

    /// What to say when a relative import names nothing.
    ///
    /// Three answers, because the useful one depends on what is around: a
    /// program in no project is being told about relative paths, and one in a
    /// project that has written something a package name would be spelled like
    /// is being told which verb it has not run.
    fn missing_file_help(&self, spec: &str) -> String {
        const RELATIVE: &str = "an import path is relative to the file it is written in";
        if !looks_like_a_package(spec) {
            return RELATIVE.into();
        }
        if self.packages.uninstalled {
            return format!(
                "{RELATIVE}; if `{spec}` is a package, this project has not been installed -- \
                 run `ingot install`"
            );
        }
        if self.packages.entries.is_empty() {
            return format!("{RELATIVE}, or one of the library's, such as `@import(\"std/http\")`");
        }
        format!(
            "{RELATIVE}; `{spec}` is not a package this project depends on -- `ingot add` \
             records one and `ingot install` makes it available"
        )
    }
}

/// A path in a form two spellings of the same file agree on.
fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// What a file module is *called*, given where it is.
///
/// A module path is an identity and a name at once: sema keys its qualified
/// names on it, a diagnostic says it out loud, and `--emit=api` prints it as
/// part of an interface tools are written against. So it is written in the one
/// spelling this project uses everywhere -- separators are `/`, as they are in
/// a lockfile, in `ingot.env`, and in every path `std/path` hands back.
///
/// [`canonical`] is what makes it an identity and is also what makes it need
/// this: on Windows `canonicalize` answers with the extended-length form,
/// `\\?\C:\Users\...`, which is a fine thing to hand to the file system and a
/// poor thing to hand to a reader. Two machines would otherwise disagree about
/// a module's name for reasons about path syntax rather than about the program.
///
/// Only on Windows. `\` is a legal character in a Unix file name, and rewriting
/// one there would be naming a different file.
pub fn module_path_of(path: &Path) -> String {
    let shown = path.display().to_string();
    if cfg!(windows) {
        windows_module_path(&shown)
    } else {
        shown
    }
}

/// [`module_path_of`]'s Windows half.
///
/// Split out, and taking a `&str`, so that it can be tested on a machine that
/// is not Windows -- which is every machine this is usually written on. `cfg!`
/// rather than `#[cfg]` above is the other half of that: both arms compile
/// everywhere, so neither can rot unnoticed.
fn windows_module_path(shown: &str) -> String {
    // `\\?\UNC\server\share` is the extended-length spelling of
    // `\\server\share`, so the prefix comes off and the two leading separators
    // go back on. Tested before the plain prefix, which it begins with.
    let stripped = match shown.strip_prefix(r"\\?\UNC\") {
        Some(rest) => format!(r"\\{rest}"),
        None => shown.strip_prefix(r"\\?\").unwrap_or(shown).to_string(),
    };
    stripped.replace('\\', "/")
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The one that matters: dependencies are the fourth field onwards, so a
    /// package name is whatever a name is and the file has one separator.
    #[test]
    fn an_environment_is_read_as_written() {
        let text = "myapp\t/work/app\tsrc/myapp.ws\tacme/json\tutil\n\
                    util\t/store/c14b\tsrc/util.ws\n";
        let entries = parse_env(text, Path::new("/work/app")).expect("a readable environment");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "myapp");
        assert_eq!(entries[0].root, Path::new("src/myapp.ws"));
        assert_eq!(entries[0].deps, vec!["acme/json", "util"]);
        // No dependencies is no further fields, not an empty one.
        assert!(entries[1].deps.is_empty());
    }

    #[test]
    fn blank_lines_are_not_packages() {
        let entries = parse_env("\n\nutil\t/store/c14b\tsrc/util.ws\n\n", Path::new("/work"))
            .expect("a readable environment");
        assert_eq!(entries.len(), 1);
    }

    /// A shape this does not know is a file from another version of ingot, and
    /// the answer to that is to write it again rather than to guess.
    #[test]
    fn a_line_that_is_not_a_package_is_refused() {
        assert!(parse_env("util\t/store/c14b\n", Path::new("/work")).is_none());
        assert!(parse_env("util\n", Path::new("/work")).is_none());
        assert!(parse_env("\t/store/c14b\tsrc/util.ws\n", Path::new("/work")).is_none());
    }

    /// A relative directory is taken against the project, so an environment
    /// written by something that did not have `os.cwd` would still resolve.
    #[test]
    fn a_relative_directory_is_the_projects() {
        let entries = parse_env("util\tvendor/util\tsrc/util.ws\n", Path::new("/work/app"))
            .expect("a readable environment");
        assert_eq!(entries[0].dir, Path::new("/work/app/vendor/util"));
    }

    /// The longest prefix wins, because `WSHARP_HOME` may sit inside the
    /// project -- which is what `ingot`'s own verb tests arrange.
    #[test]
    fn the_owner_of_a_file_is_the_innermost_package() {
        let packages = Packages {
            entries: parse_env(
                "myapp\t/work/app\tsrc/myapp.ws\tutil\n\
                 util\t/work/app/home/store/sha256/c14b\tsrc/util.ws\n",
                Path::new("/work/app"),
            )
            .expect("a readable environment"),
            uninstalled: false,
        };
        let inside = Path::new("/work/app/home/store/sha256/c14b/src/util.ws");
        assert_eq!(
            packages.owner_of(inside).map(|p| p.name.as_str()),
            Some("util")
        );
        let own = Path::new("/work/app/src/myapp.ws");
        assert_eq!(
            packages.owner_of(own).map(|p| p.name.as_str()),
            Some("myapp")
        );
        // Reached by a relative path out of the project: the project's own.
        let outside = Path::new("/elsewhere/shared.ws");
        assert_eq!(
            packages.owner_of(outside).map(|p| p.name.as_str()),
            Some("myapp")
        );
    }

    /// A hint for choosing a help line, and nothing more.
    #[test]
    fn a_package_is_spelled_unlike_a_relative_path() {
        assert!(looks_like_a_package("acme/json"));
        assert!(looks_like_a_package("util"));
        assert!(!looks_like_a_package("./modules/geometry.ws"));
        assert!(!looks_like_a_package("../util.ws"));
    }

    /// The Windows spelling of a module path, checked from Linux.
    ///
    /// This is the whole reason [`windows_module_path`] takes a `&str` instead
    /// of being written inline under a `#[cfg]`: the rule is a fact about
    /// strings, and a fact about strings can be checked on the machine this is
    /// written on rather than only on the runner that found it.
    #[test]
    fn a_windows_module_path_is_written_with_forward_slashes() {
        assert_eq!(
            windows_module_path(r"\\?\C:\Users\ofek\app\fw.ws"),
            "C:/Users/ofek/app/fw.ws",
            "the extended-length prefix comes off and the separators turn"
        );
        // Already short: `canonicalize` produces the long form, but nothing
        // here may assume that is the only thing it will ever be handed.
        assert_eq!(windows_module_path(r"C:\app\fw.ws"), "C:/app/fw.ws");
        // A share. `\\?\UNC\server\share` *is* `\\server\share`, so the two
        // leading separators have to survive -- and they are the reason the
        // UNC prefix is tested before the plain one it begins with.
        assert_eq!(
            windows_module_path(r"\\?\UNC\server\share\app\fw.ws"),
            "//server/share/app/fw.ws"
        );
        // Nothing to do, and nothing done.
        assert_eq!(windows_module_path("std/net"), "std/net");
    }
}
