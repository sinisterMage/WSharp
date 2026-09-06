//! An s-expression dump of the AST, used by `wsharp --emit=ast` and by the
//! parser tests (comparing one compact string beats matching nested enums).
//!
//! Expressions print inline; statements get a line each.

use crate::ast::*;

pub fn dump_module(module: &Module) -> String {
    let mut p = Printer {
        out: String::new(),
        indent: 0,
    };
    p.line("(module");
    p.indent += 1;
    for item in &module.items {
        p.item(item);
    }
    p.indent -= 1;
    p.push_close();
    p.out
}

struct Printer {
    out: String,
    indent: usize,
}

impl Printer {
    fn line(&mut self, text: &str) {
        for _ in 0..self.indent {
            self.out.push_str("  ");
        }
        self.out.push_str(text);
        self.out.push('\n');
    }

    /// Append a `)` to the previous line rather than putting it on its own.
    fn push_close(&mut self) {
        while self.out.ends_with('\n') {
            self.out.pop();
        }
        self.out.push(')');
        self.out.push('\n');
    }

    fn item(&mut self, item: &Item) {
        match item {
            Item::Struct(s) => {
                let parent = s
                    .parent
                    .as_ref()
                    .map(|p| format!(" (parent {p})"))
                    .unwrap_or_default();
                // Zero-field structs are ordinary now -- they are how the
                // dispatch lattice is written -- so do not leave a dangling
                // space where the field list would have been.
                let fields: String = s
                    .fields
                    .iter()
                    .map(|f| format!(" ({} {})", f.name, ty(&f.ty)))
                    .collect();
                self.line(&format!(
                    "({}struct {}{}{}{}",
                    vis(s.is_public),
                    s.name,
                    generics(&s.generics),
                    parent,
                    fields
                ));
                self.push_close();
            }
            Item::Const(c) => {
                let annot =
                    c.ty.as_ref()
                        .map(|t| format!(" : {}", ty(t)))
                        .unwrap_or_default();
                self.line(&format!(
                    "({}const {}{} {}",
                    vis(c.is_public),
                    c.name,
                    annot,
                    expr(&c.value)
                ));
                self.push_close();
            }
            Item::Fn(f) => {
                self.line(&format!(
                    "({}fn {}{} {}",
                    vis(f.is_public),
                    f.name,
                    generics(&f.func.generics),
                    sig(&f.func)
                ));
                self.indent += 1;
                self.stmts(&f.func.body);
                self.indent -= 1;
                self.push_close();
            }
        }
    }

    fn stmts(&mut self, block: &Block) {
        for stmt in &block.stmts {
            self.stmt(stmt);
        }
    }

    fn nested(&mut self, block: &Block) {
        self.indent += 1;
        if block.stmts.is_empty() {
            self.line("(block)");
        } else {
            self.line("(block");
            self.indent += 1;
            self.stmts(block);
            self.indent -= 1;
            self.push_close();
        }
        self.indent -= 1;
    }

    fn stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Let(l) => {
                let kw = if l.mutable { "var" } else { "const" };
                let annot =
                    l.ty.as_ref()
                        .map(|t| format!(" : {}", ty(t)))
                        .unwrap_or_default();
                self.line(&format!("({kw} {}{} {})", l.name, annot, expr(&l.init)));
            }
            Stmt::Assign(a) => {
                let op = a.op.map(|o| o.text()).unwrap_or("");
                self.line(&format!(
                    "(assign{} {} {})",
                    op,
                    expr(&a.target),
                    expr(&a.value)
                ));
            }
            Stmt::Expr(e) => self.line(&expr(e)),
            Stmt::Return { value: Some(v), .. } => self.line(&format!("(return {})", expr(v))),
            Stmt::Return { value: None, .. } => self.line("(return)"),
            Stmt::Break(_) => self.line("(break)"),
            Stmt::Continue(_) => self.line("(continue)"),
            Stmt::Block(b) => {
                self.line("(block");
                self.indent += 1;
                self.stmts(b);
                self.indent -= 1;
                self.push_close();
            }
            Stmt::If(i) => self.if_stmt(i),
            Stmt::While(w) => {
                let cap = w
                    .capture
                    .as_ref()
                    .map(|c| format!(" |{c}|"))
                    .unwrap_or_default();
                self.line(&format!("(while {}{}", expr(&w.cond), cap));
                if let Some(cont) = &w.cont {
                    self.indent += 1;
                    self.line("(continue-expr");
                    self.indent += 1;
                    self.stmt(cont);
                    self.indent -= 1;
                    self.push_close();
                    self.indent -= 1;
                }
                self.nested(&w.body);
                self.push_close();
            }

            Stmt::For(f) => {
                let cap = match &f.index {
                    Some(index) => format!(" |{} {}|", f.value, index),
                    None => format!(" |{}|", f.value),
                };
                self.line(&format!("(for {}{}", expr(&f.iter), cap));
                self.nested(&f.body);
                self.push_close();
            }
        }
    }

    fn if_stmt(&mut self, i: &IfStmt) {
        let cap = i
            .capture
            .as_ref()
            .map(|c| format!(" |{c}|"))
            .unwrap_or_default();
        self.line(&format!("(if {}{}", expr(&i.cond), cap));
        self.nested(&i.then);
        match i.else_.as_deref() {
            Some(ElseBranch::Block(b)) => {
                self.indent += 1;
                self.line("(else");
                self.nested(b);
                self.push_close();
                self.indent -= 1;
            }
            Some(ElseBranch::If(inner)) => {
                self.indent += 1;
                self.line("(else");
                self.indent += 1;
                self.if_stmt(inner);
                self.indent -= 1;
                self.push_close();
                self.indent -= 1;
            }
            None => {}
        }
        self.push_close();
    }
}

/// `pub ` for a declaration another module may name, and nothing otherwise --
/// so every dump written before visibility existed still reads the same.
fn vis(is_public: bool) -> &'static str {
    if is_public { "pub " } else { "" }
}

/// ` [T U]` for a declaration's type parameters, or nothing when it has none.
fn generics(names: &[crate::span::Ident]) -> String {
    if names.is_empty() {
        return String::new();
    }
    let names: Vec<String> = names.iter().map(|n| n.to_string()).collect();
    format!(" [{}]", names.join(" "))
}

fn sig(f: &Func) -> String {
    let params: Vec<String> = f
        .params
        .iter()
        .map(|p| match &p.ty {
            Some(t) => format!("({} {})", p.name, ty(t)),
            None => format!("({})", p.name),
        })
        .collect();
    let ret = f
        .ret
        .as_ref()
        .map(|t| format!(" (ret {})", ty(t)))
        .unwrap_or_default();
    format!("(params {}){}", params.join(" "), ret)
}

fn ty(t: &TypeExpr) -> String {
    match t {
        TypeExpr::Named(id) => id.to_string(),
        TypeExpr::Array { elem, .. } => format!("[]{}", ty(elem)),
        TypeExpr::Path { segments, args, .. } => {
            let path: Vec<String> = segments.iter().map(|s| s.to_string()).collect();
            let path = path.join(".");
            if args.is_empty() {
                path
            } else {
                let args: Vec<String> = args.iter().map(ty).collect();
                format!("{path}[{}]", args.join(", "))
            }
        }
        TypeExpr::Optional { inner, .. } => format!("?{}", ty(inner)),
        TypeExpr::ErrUnion { inner, .. } => format!("!{}", ty(inner)),
        TypeExpr::Fn { params, ret, .. } => {
            let ps: Vec<String> = params.iter().map(ty).collect();
            format!("fn({}) {}", ps.join(", "), ty(ret))
        }
    }
}

pub fn expr(e: &Expr) -> String {
    match e {
        Expr::Int(v, _) => v.to_string(),
        Expr::Float(v, _) => {
            // Always show a decimal point so `1.0` is not confused with `1`.
            if v.fract() == 0.0 && v.is_finite() {
                format!("{v:.1}")
            } else {
                v.to_string()
            }
        }
        Expr::Bool(v, _) => v.to_string(),
        Expr::Str(s, _) => format!("{:?}", s.as_ref()),
        Expr::Null(_) => "null".into(),
        Expr::Ident(id) => id.to_string(),
        Expr::ErrorLit { name, .. } => format!("error.{name}"),
        Expr::Unary {
            op, expr: inner, ..
        } => format!("({} {})", op.text(), expr(inner)),
        Expr::Binary { op, lhs, rhs, .. } => {
            format!("({} {} {})", op.text(), expr(lhs), expr(rhs))
        }
        Expr::Call { callee, args, .. } => {
            let mut parts = vec![format!("call {}", expr(callee))];
            parts.extend(args.iter().map(expr));
            format!("({})", parts.join(" "))
        }
        Expr::Field { obj, name, .. } => format!("(. {} {})", expr(obj), name),
        Expr::ArrayLit { elem, elems, .. } => {
            let es: Vec<String> = elems.iter().map(expr).collect();
            format!("(array {} {})", ty(elem), es.join(" "))
        }
        Expr::Index { obj, index, .. } => format!("(index {} {})", expr(obj), expr(index)),
        Expr::Import { path, .. } => format!("(import {path:?})"),
        Expr::StructLit { path, fields, .. } => {
            let fs: Vec<String> = fields
                .iter()
                .map(|f| format!("({} {})", f.name, expr(&f.value)))
                .collect();
            let path: Vec<String> = path.iter().map(|p| p.to_string()).collect();
            format!("(lit {} {})", path.join("."), fs.join(" "))
        }
        Expr::Fn(f) => format!("(fn{} {} ...)", generics(&f.generics), sig(f)),
        Expr::If(i) => {
            let cap = i
                .capture
                .as_ref()
                .map(|c| format!(" |{c}|"))
                .unwrap_or_default();
            format!(
                "(if{} {} {} {})",
                cap,
                expr(&i.cond),
                expr(&i.then),
                expr(&i.else_)
            )
        }
        Expr::Try { expr: inner, .. } => format!("(try {})", expr(inner)),
        Expr::Catch {
            expr: inner,
            capture,
            alt,
            ..
        } => {
            let cap = capture
                .as_ref()
                .map(|c| format!(" |{c}|"))
                .unwrap_or_default();
            format!("(catch{} {} {})", cap, expr(inner), expr(alt))
        }
        Expr::Orelse {
            expr: inner, alt, ..
        } => {
            format!("(orelse {} {})", expr(inner), expr(alt))
        }
        Expr::Unwrap { expr: inner, .. } => format!("(unwrap {})", expr(inner)),
    }
}
