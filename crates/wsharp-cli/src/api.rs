//! `--emit=api`: a program's declared surface, for a tool to read.
//!
//! `--emit=ast` exists too, and is deliberately *not* this. It is a debugging
//! aid shared with the parser tests, it prints whatever the syntax tree happens
//! to hold, it renames a node whenever the parser does, and it says nothing
//! about which module a qualified name reaches. A generator written against it
//! is written against something with no promise attached.
//!
//! So this is the promise, and it is a narrow one on purpose:
//!
//! * **It is versioned.** The first line is `(api 1)`. A change that could make
//!   an existing reader wrong bumps the number.
//! * **It is a surface, not a program.** Declarations and their types; no
//!   bodies, no expressions, no spans.
//! * **Every name is resolved.** A type written `raython.Route` is printed
//!   `"raython/web".Route`, and one declared here is printed with this module's
//!   own path -- so every name that is not built into the language is absolute,
//!   and a reader never follows an import or guesses a scope. That is the half
//!   `--emit=ast` leaves undone and the half that is easy to get wrong.
//! * **It describes a program the compiler accepted.** It is printed after type
//!   checking, so a tool generating code from it never has to wonder whether
//!   what it read type-checks.
//!
//! S-expressions rather than JSON, because that is what the reader on the other
//! end is: a few hundred lines with no dependency, in a language whose `std`
//! has no JSON either.
//!
//! A module is named by the path the loader resolved it to, which for a library
//! module is `std/net` and for a file is that file. The one exception is the
//! root, which is `"main"` -- the loader's name for it, and what every other
//! emit already calls it.
//!
//! ```text
//! (api 1)
//! (module "app/controllers/users"
//!   (import raython "raython/web")
//!   (pub const ROUTE_Index (str "GET /users"))
//!   (pub struct Index (parent "raython/web".Route)
//!     (field page i64))
//!   (pub fn action (param c "raython/web".Ctx) (param r "app/controllers/users".Index)
//!     (ret "raython/web".Model)))
//! ```

use std::collections::{HashMap, HashSet};

use wsharp_syntax::ast;

/// The shape of what is printed. Bumped when a change could make an existing
/// reader wrong; adding a new form inside an existing one does not, because a
/// reader that does not know a form skips it.
pub const VERSION: u32 = 1;

/// One module as the emitter sees it: what it is called, and what its local
/// names for other modules mean.
///
/// `specifiers` is the loader's answer to "what does this `@import` string
/// resolve to", which is a fact about the file system. The local *name* for a
/// module is a different question -- it is whatever the `const` was called --
/// and is worked out here, because that is the half a reader needs and the half
/// `--emit=ast` leaves it to do.
pub struct Module<'a> {
    pub path: &'a str,
    pub ast: &'a ast::Module,
    pub specifiers: &'a HashMap<String, String>,
}

impl Module<'_> {
    /// Every name this module declares, so that a bare one in a type can be
    /// told from a name the language provides. `i64` and `Number` are not here
    /// and stay bare; `Show` is and is printed with this module's path.
    fn declared(&self) -> HashSet<String> {
        self.ast
            .items
            .iter()
            .map(|i| i.name().to_string())
            .collect()
    }

    /// `const core = @import("./fw/core.ws");` -> `core` means `"probes/fw/core"`.
    fn aliases(&self) -> HashMap<String, String> {
        let mut out = HashMap::new();
        for item in &self.ast.items {
            let ast::Item::Const(d) = item else { continue };
            let ast::Expr::Import { path, .. } = &d.value else {
                continue;
            };
            let resolved = self
                .specifiers
                .get(path.as_ref())
                .cloned()
                .unwrap_or_else(|| path.to_string());
            out.insert(d.name.to_string(), resolved);
        }
        out
    }
}

pub fn emit(modules: &[Module<'_>]) -> String {
    let known: Vec<&str> = modules.iter().map(|m| m.path).collect();
    let mut out = format!("(api {VERSION})\n");
    for module in modules {
        out.push_str(&emit_module(module, &known));
    }
    out
}

fn emit_module(module: &Module<'_>, known: &[&str]) -> String {
    let aliases = module.aliases();
    let scope = Scope {
        here: module.path,
        declared: module.declared(),
        aliases,
    };
    let mut out = format!("(module {}\n", quoted(module.path));
    // Sorted, because a `HashMap` has no order and a diff of two emits should
    // be about the program rather than about the hasher.
    let mut imports: Vec<(&String, &String)> = scope.aliases.iter().collect();
    imports.sort();
    for (alias, path) in imports {
        out.push_str(&format!("  (import {alias} {})\n", quoted(path)));
    }
    for item in &module.ast.items {
        out.push_str(&emit_item(item, &scope, known));
    }
    // The closing paren joins the last line, as `dump` does.
    let trimmed = out.trim_end().to_string();
    format!("{trimmed})\n")
}

/// What a name written in one module means.
struct Scope<'a> {
    /// This module's own path, which every name it declares is printed with.
    here: &'a str,
    declared: HashSet<String>,
    aliases: HashMap<String, String>,
}

fn emit_item(item: &ast::Item, scope: &Scope<'_>, known: &[&str]) -> String {
    // An `@import` is already the `(import ...)` line above; printing the
    // `const` again would be one fact in two places.
    if let ast::Item::Const(d) = item
        && matches!(d.value, ast::Expr::Import { .. })
    {
        return String::new();
    }
    match item {
        ast::Item::Struct(d) => {
            let vis = visibility(d.is_public);
            let generics = generics(&d.generics);
            let mut out = format!("  ({vis}struct {}{generics}", d.name);
            if let Some(parent) = &d.parent {
                out.push_str(&format!(" (parent {})", ty(parent, scope, known)));
            }
            for field in &d.fields {
                out.push_str(&format!(
                    "\n    (field {} {})",
                    field.name,
                    ty(&field.ty, scope, known)
                ));
            }
            format!("{out})\n")
        }
        ast::Item::Fn(d) => {
            let vis = visibility(d.is_public);
            let generics = generics(&d.func.generics);
            let mut out = format!("  ({vis}fn {}{generics}", d.name);
            for param in &d.func.params {
                let shown = match &param.ty {
                    Some(t) => ty(t, scope, known),
                    // Left to inference, which is a fact about the declaration
                    // and not something to invent a type for here.
                    None => "?".to_string(),
                };
                out.push_str(&format!("\n    (param {} {shown})", param.name));
            }
            if let Some(ret) = &d.func.ret {
                out.push_str(&format!("\n    (ret {})", ty(ret, scope, known)));
            }
            format!("{out})\n")
        }
        ast::Item::Const(d) => {
            let vis = visibility(d.is_public);
            let mut out = format!("  ({vis}const {}", d.name);
            if let Some(t) = &d.ty {
                out.push_str(&format!(" (type {})", ty(t, scope, known)));
            }
            out.push_str(&value(&d.value, scope, known));
            format!("{out})\n")
        }
    }
}

fn visibility(is_public: bool) -> &'static str {
    if is_public { "pub " } else { "" }
}

fn generics(names: &[wsharp_syntax::Ident]) -> String {
    if names.is_empty() {
        return String::new();
    }
    let shown: Vec<String> = names.iter().map(|n| n.to_string()).collect();
    format!(" (generics {})", shown.join(" "))
}

/// What a `const` is bound to, as far as it is part of a surface.
///
/// Top-level `const` may bind only a literal, a `fn` or another module's name,
/// and each of those three is something a tool has a use for: a literal is the
/// escape hatch a route or a limit is written as, a `fn` is a declaration in
/// another spelling, and a name is a re-export. Anything else prints as
/// `(other)` rather than as a guess.
fn value(expr: &ast::Expr, scope: &Scope<'_>, known: &[&str]) -> String {
    match expr {
        ast::Expr::Str(value, _) => format!(" (str {})", quoted(value)),
        ast::Expr::Int(value, _) => format!(" (int {value})"),
        ast::Expr::Float(value, _) => format!(" (float {value})"),
        ast::Expr::Bool(value, _) => format!(" (bool {value})"),
        ast::Expr::Import { path, .. } => format!(" (import {})", quoted(path)),
        ast::Expr::Fn(func) => {
            let mut out = String::from(" (fn");
            out.push_str(&generics(&func.generics));
            for param in &func.params {
                let shown = match &param.ty {
                    Some(t) => ty(t, scope, known),
                    None => "?".to_string(),
                };
                out.push_str(&format!("\n    (param {} {shown})", param.name));
            }
            if let Some(ret) = &func.ret {
                out.push_str(&format!("\n    (ret {})", ty(ret, scope, known)));
            }
            format!("{out})")
        }
        // `pub const T = other.T;` -- a second name for one thing rather than a
        // copy of it, which is the whole of how a package presents one file.
        // Written as a field access, because that is what `other.T` parses as.
        _ => match dotted(expr) {
            Some(segments) => format!(" (alias {})", resolve(&segments, scope, known)),
            None => " (other)".to_string(),
        },
    }
}

/// A chain of field accesses over a bare name, as the segments it spells.
///
/// `other.T` is `Field { obj: Ident("other"), name: "T" }`, and a re-export
/// through a nested module is one more of those.
fn dotted(expr: &ast::Expr) -> Option<Vec<wsharp_syntax::Ident>> {
    match expr {
        ast::Expr::Ident(name) => Some(vec![name.clone()]),
        ast::Expr::Field { obj, name, .. } => {
            let mut out = dotted(obj)?;
            out.push(name.clone());
            Some(out)
        }
        _ => None,
    }
}

/// A type, with every module reference resolved to the module's own path.
fn ty(t: &ast::TypeExpr, scope: &Scope<'_>, known: &[&str]) -> String {
    match t {
        ast::TypeExpr::Named(name) => resolve(std::slice::from_ref(name), scope, known),
        ast::TypeExpr::Array { elem, .. } => format!("(array {})", ty(elem, scope, known)),
        ast::TypeExpr::Optional { inner, .. } => {
            format!("(optional {})", ty(inner, scope, known))
        }
        ast::TypeExpr::ErrUnion { inner, errors, .. } => {
            let set = match errors {
                // An inferred set is not written here rather than written as
                // empty: "the compiler works it out" and "it raises nothing"
                // are different statements.
                None => String::new(),
                Some(names) => {
                    let shown: Vec<String> = names.iter().map(|n| n.to_string()).collect();
                    format!(" (errors {})", shown.join(" "))
                }
            };
            format!("(errunion {}{set})", ty(inner, scope, known))
        }
        ast::TypeExpr::Fn { params, ret, .. } => {
            let shown: Vec<String> = params.iter().map(|p| ty(p, scope, known)).collect();
            format!("(fn ({}) {})", shown.join(" "), ty(ret, scope, known))
        }
        ast::TypeExpr::Path { segments, args, .. } => {
            let base = resolve(segments, scope, known);
            if args.is_empty() {
                return base;
            }
            let shown: Vec<String> = args.iter().map(|a| ty(a, scope, known)).collect();
            format!("(at {base} {})", shown.join(" "))
        }
    }
}

/// A dotted name, with its leading module segments replaced by the module's
/// own path.
///
/// The rule is the compiler's: the first segment may name an import, and each
/// further segment may extend that module path, because a module path is a
/// prefix -- `std.http.NotFound404` reaches `std/http` through an import of
/// `std`. What is left after the longest module prefix is the name.
///
/// A name that reaches no module at all prints bare, which is what a built-in
/// type and a type declared here both are; a reader tells them apart by looking
/// for the name among this module's own declarations, exactly as the compiler
/// does.
fn resolve(segments: &[wsharp_syntax::Ident], scope: &Scope<'_>, known: &[&str]) -> String {
    let first = segments[0].to_string();
    let Some(base) = scope.aliases.get(&first) else {
        // Not a module, so it is either a name this module declares -- printed
        // with this module's path, so that every name a program defines is
        // absolute -- or one the language provides, which stays bare.
        let shown: Vec<String> = segments.iter().map(|s| s.to_string()).collect();
        if shown.len() == 1 && scope.declared.contains(&shown[0]) {
            return format!("{}.{}", quoted(scope.here), shown[0]);
        }
        return shown.join(".");
    };
    let mut path = base.clone();
    let mut at = 1;
    while at + 1 < segments.len() {
        let extended = format!("{path}/{}", segments[at]);
        if !known.contains(&extended.as_str()) {
            break;
        }
        path = extended;
        at += 1;
    }
    let rest: Vec<String> = segments[at..].iter().map(|s| s.to_string()).collect();
    if rest.is_empty() {
        return quoted(&path);
    }
    format!("{}.{}", quoted(&path), rest.join("."))
}

/// A string, with the two characters that would end it escaped.
fn quoted(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}
