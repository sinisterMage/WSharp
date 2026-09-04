//! Recursive-descent parser with a precedence-climbing expression parser.
//!
//! Precedence follows Zig's table, loosest first:
//!
//! ```text
//!   or
//!   and
//!   == != < <= > >=      (non-associative)
//!   orelse  catch
//!   + -
//!   * / %
//!   - ! try              (prefix)
//!   f()  .field  .?      (postfix)
//! ```
//!
//! On an error the parser records a diagnostic and resynchronises at the next
//! statement or item boundary, so one mistake does not cascade.

use crate::ast::*;
use crate::diag::Diagnostic;
use crate::span::{Ident, Span};
use crate::token::{Token, TokenKind};

/// Parse a source file. The module is returned even when there were errors (it
/// simply omits the parts that could not be parsed), so later passes can still
/// run and report more.
pub fn parse(src: &str) -> (Module, Vec<Diagnostic>) {
    parse_at(src, 0)
}

/// Parse one file of a program, with every span offset by `base`.
///
/// The parser itself needs no change: it only ever copies spans out of the
/// tokens the lexer produced, which already carry the offset.
pub fn parse_at(src: &str, base: u32) -> (Module, Vec<Diagnostic>) {
    let (tokens, mut diags) = crate::lexer::lex_at(src, base);
    let mut parser = Parser {
        tokens,
        pos: 0,
        diags: Vec::new(),
        depth: 0,
        too_deep: false,
    };
    let module = parser.module();
    diags.append(&mut parser.diags);
    (module, diags)
}

/// How deeply expressions, types and blocks may nest.
///
/// The parser is recursive, so unbounded nesting is a stack overflow -- a
/// crash with no diagnostic. The limit is far beyond anything written by hand
/// and well inside what the stack holds, including for the passes downstream,
/// whose recursion mirrors the tree this one builds.
pub const MAX_DEPTH: u32 = 256;

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    diags: Vec<Diagnostic>,
    /// Current nesting level, bounded by [`MAX_DEPTH`].
    depth: u32,
    /// Whether the limit has been reported. Once is enough: every deeper
    /// level would say the same thing.
    too_deep: bool,
}

impl Parser {
    // ---- token plumbing -------------------------------------------------

    fn peek(&self) -> &TokenKind {
        &self.tokens[self.pos].kind
    }

    fn span(&self) -> Span {
        self.tokens[self.pos].span
    }

    fn prev_span(&self) -> Span {
        self.tokens[self.pos.saturating_sub(1)].span
    }

    fn at(&self, kind: &TokenKind) -> bool {
        std::mem::discriminant(self.peek()) == std::mem::discriminant(kind)
    }

    fn at_eof(&self) -> bool {
        matches!(self.peek(), TokenKind::Eof)
    }

    fn bump(&mut self) -> Token {
        let token = self.tokens[self.pos].clone();
        if !matches!(token.kind, TokenKind::Eof) {
            self.pos += 1;
        }
        token
    }

    fn eat(&mut self, kind: TokenKind) -> bool {
        if self.at(&kind) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, kind: TokenKind) -> Option<Token> {
        if self.at(&kind) {
            Some(self.bump())
        } else {
            let found = self.peek().describe();
            let expected = kind.describe();
            self.error(self.span(), format!("expected {expected}, found {found}"));
            None
        }
    }

    fn ident(&mut self) -> Option<Ident> {
        match self.peek().clone() {
            TokenKind::Ident(name) => {
                let span = self.span();
                self.bump();
                Some(Ident::new(name, span))
            }
            other => {
                self.error(
                    self.span(),
                    format!("expected an identifier, found {}", other.describe()),
                );
                None
            }
        }
    }

    fn error(&mut self, span: Span, message: impl Into<String>) {
        self.push_error(Diagnostic::error(span, message));
    }

    /// An error with a help line. The help goes on this diagnostic only: if
    /// it is dropped as an echo, the help must not land on whatever was
    /// reported before it.
    fn error_with_help(&mut self, span: Span, message: impl Into<String>, help: impl Into<String>) {
        self.push_error(Diagnostic::error(span, message).help(help));
    }

    fn push_error(&mut self, diag: Diagnostic) {
        // Only the first error at a given position is useful; the rest are echoes.
        if self
            .diags
            .last()
            .is_some_and(|d| d.primary.span == diag.primary.span)
        {
            return;
        }
        self.diags.push(diag);
    }

    /// Enter one nesting level, or report that the input is too deep and
    /// refuse.
    ///
    /// Levels are released by [`Parser::scoped`] rather than one at a time,
    /// because a chain of binary or postfix operators nests its *left* operand
    /// one level per link even though the loop that parses it never recurses.
    /// Entering once per link and releasing at the end of the chain counts
    /// that nesting the way every later pass will walk it.
    fn enter(&mut self, what: &str) -> Option<()> {
        if self.depth >= MAX_DEPTH {
            if !self.too_deep {
                self.too_deep = true;
                self.error_with_help(
                    self.span(),
                    format!("{what} is nested too deeply"),
                    format!("the limit is {MAX_DEPTH} levels"),
                );
            }
            return None;
        }
        self.depth += 1;
        Some(())
    }

    /// Run `f`, then release every level it entered.
    fn scoped<T>(&mut self, f: impl FnOnce(&mut Parser) -> T) -> T {
        let base = self.depth;
        let result = f(self);
        self.depth = base;
        result
    }

    // ---- recovery -------------------------------------------------------

    /// Skip tokens until something that plausibly starts a new item.
    fn recover_to_item(&mut self) {
        let mut depth = 0usize;
        while !self.at_eof() {
            match self.peek() {
                TokenKind::LBrace => depth += 1,
                TokenKind::RBrace => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        self.bump();
                        return;
                    }
                }
                TokenKind::Fn | TokenKind::Const | TokenKind::Var if depth == 0 => return,
                _ => {}
            }
            self.bump();
        }
    }

    /// Skip to just past the next `;`, or to a `}` that closes the block.
    fn recover_to_stmt(&mut self) {
        let mut depth = 0usize;
        while !self.at_eof() {
            match self.peek() {
                TokenKind::Semi if depth == 0 => {
                    self.bump();
                    return;
                }
                TokenKind::LBrace => depth += 1,
                TokenKind::RBrace => {
                    if depth == 0 {
                        return;
                    }
                    depth -= 1;
                }
                _ => {}
            }
            self.bump();
        }
    }

    // ---- items ----------------------------------------------------------

    fn module(&mut self) -> Module {
        let mut items = Vec::new();
        while !self.at_eof() {
            let before = self.pos;
            match self.item() {
                Some(item) => items.push(item),
                None => self.recover_to_item(),
            }
            if self.pos == before {
                // Recovery made no progress -- force it so we cannot spin.
                self.bump();
            }
        }
        Module { items }
    }

    fn item(&mut self) -> Option<Item> {
        match self.peek() {
            TokenKind::Fn => self.fn_decl().map(Item::Fn),
            TokenKind::Const => self.const_or_struct(),
            TokenKind::Var => {
                let span = self.span();
                self.error_with_help(
                    span,
                    "`var` is not allowed at the top level",
                    "top-level bindings must be `const`",
                );
                None
            }
            other => {
                let found = other.describe();
                self.error_with_help(
                    self.span(),
                    format!("expected a declaration, found {found}"),
                    "a W# file contains `fn` and `const` declarations",
                );
                None
            }
        }
    }

    fn fn_decl(&mut self) -> Option<FnDecl> {
        let start = self.span();
        self.expect(TokenKind::Fn)?;
        let name = self.ident()?;
        let generics = self.generic_params()?;
        let func = self.func_rest(start, generics)?;
        let span = start.to(func.span);
        Some(FnDecl { name, func, span })
    }

    /// `[T, U]` naming a declaration's or a literal's type parameters.
    ///
    /// Unambiguous wherever it appears: what precedes it is always followed by
    /// something fixed -- `(` for a function, `{` for a struct body -- so a `[`
    /// here can only start a type parameter list. Indexing is an *expression*,
    /// and no expression is expected at any of those positions. That holds for
    /// a `fn` literal too: `fn` is followed by `[` or `(`, never a value.
    fn generic_params(&mut self) -> Option<Vec<Ident>> {
        if !self.at(&TokenKind::LBracket) {
            return Some(Vec::new());
        }
        self.scoped(|p| {
            p.enter("type parameter list")?;
            p.expect(TokenKind::LBracket)?;
            let mut names = Vec::new();
            while !p.at(&TokenKind::RBracket) && !p.at_eof() {
                names.push(p.ident()?);
                if !p.eat(TokenKind::Comma) {
                    break;
                }
            }
            p.expect(TokenKind::RBracket)?;
            if names.is_empty() {
                p.error_with_help(
                    p.prev_span(),
                    "a type parameter list cannot be empty",
                    "write `[T]`, or drop the brackets entirely",
                );
            }
            Some(names)
        })
    }

    /// The `(params) RetType { body }` tail shared by `fn` declarations and
    /// `fn` literals. `start` is the span of the `fn` keyword, and `generics`
    /// whatever [`Parser::generic_params`] found before it.
    fn func_rest(&mut self, start: Span, generics: Vec<Ident>) -> Option<Func> {
        self.expect(TokenKind::LParen)?;
        let mut params = Vec::new();
        while !self.at(&TokenKind::RParen) && !self.at_eof() {
            let p_start = self.span();
            let name = self.ident()?;
            let ty = if self.eat(TokenKind::Colon) {
                Some(self.type_expr()?)
            } else {
                None
            };
            params.push(Param {
                name,
                ty,
                span: p_start.to(self.prev_span()),
            });
            if !self.eat(TokenKind::Comma) {
                break;
            }
        }
        self.expect(TokenKind::RParen)?;

        // A return type is present unless the body starts immediately. Type
        // syntax never begins with `{`, so this is unambiguous.
        let ret = if self.at(&TokenKind::LBrace) {
            None
        } else {
            Some(self.type_expr()?)
        };
        let body = self.block()?;
        let span = start.to(body.span);
        Some(Func {
            generics,
            params,
            ret,
            body,
            span,
        })
    }

    fn const_or_struct(&mut self) -> Option<Item> {
        let start = self.span();
        self.expect(TokenKind::Const)?;
        let name = self.ident()?;
        let ty = if self.eat(TokenKind::Colon) {
            Some(self.type_expr()?)
        } else {
            None
        };
        self.expect(TokenKind::Eq)?;

        if self.at(&TokenKind::Struct) {
            if let Some(ty) = &ty {
                self.error(
                    ty.span(),
                    "a struct declaration cannot have a type annotation",
                );
            }
            self.expect(TokenKind::Struct)?;
            let generics = self.generic_params()?;
            // `struct : Parent { ... }` declares a subtype. The colon is
            // unambiguous here: the annotation slot before `=` was already
            // consumed above, and a struct body always starts with `{`.
            let parent = if self.eat(TokenKind::Colon) {
                Some(self.ident()?)
            } else {
                None
            };
            // Type ids are a preorder walk of the lattice, fixed before
            // monomorphisation, and a generic struct's instantiations are not
            // known until after it -- so a generic struct stands outside the
            // lattice entirely.
            if let (false, Some(parent)) = (generics.is_empty(), &parent) {
                self.error_with_help(
                    parent.span,
                    "a generic struct cannot have a supertype",
                    "give the subtype concrete fields, or drop the type parameters",
                );
            }
            let fields = self.struct_body()?;
            self.expect(TokenKind::Semi)?;
            let span = start.to(self.prev_span());
            return Some(Item::Struct(StructDecl {
                name,
                generics,
                parent,
                fields,
                span,
            }));
        }

        let value = self.expr()?;
        self.expect(TokenKind::Semi)?;
        let span = start.to(self.prev_span());
        Some(Item::Const(ConstDecl {
            name,
            ty,
            value,
            span,
        }))
    }

    /// The `{ name: Type, ... }` body of a struct declaration. The `struct`
    /// keyword and any `: Parent` are consumed by the caller.
    fn struct_body(&mut self) -> Option<Vec<FieldDecl>> {
        self.expect(TokenKind::LBrace)?;
        let mut fields = Vec::new();
        while !self.at(&TokenKind::RBrace) && !self.at_eof() {
            let f_start = self.span();
            let name = self.ident()?;
            self.expect(TokenKind::Colon)?;
            let ty = self.type_expr()?;
            fields.push(FieldDecl {
                name,
                ty,
                span: f_start.to(self.prev_span()),
            });
            if !self.eat(TokenKind::Comma) {
                break;
            }
        }
        self.expect(TokenKind::RBrace)?;
        Some(fields)
    }

    // ---- types ----------------------------------------------------------

    fn type_expr(&mut self) -> Option<TypeExpr> {
        self.scoped(|p| {
            p.enter("type")?;
            p.type_expr_inner()
        })
    }

    fn type_expr_inner(&mut self) -> Option<TypeExpr> {
        let start = self.span();
        match self.peek() {
            // `[]T`. The empty brackets are Zig's: a slice has no length in
            // its type, and the object carries it instead.
            TokenKind::LBracket => {
                self.bump();
                self.expect(TokenKind::RBracket)?;
                let elem = self.type_expr()?;
                Some(TypeExpr::Array {
                    elem: Box::new(elem),
                    span: start.to(self.prev_span()),
                })
            }
            TokenKind::Question => {
                self.bump();
                let inner = self.type_expr()?;
                Some(TypeExpr::Optional {
                    span: start.to(inner.span()),
                    inner: Box::new(inner),
                })
            }
            TokenKind::Bang => {
                self.bump();
                let inner = self.type_expr()?;
                Some(TypeExpr::ErrUnion {
                    span: start.to(inner.span()),
                    inner: Box::new(inner),
                })
            }
            TokenKind::Fn => {
                self.bump();
                self.expect(TokenKind::LParen)?;
                let mut params = Vec::new();
                while !self.at(&TokenKind::RParen) && !self.at_eof() {
                    params.push(self.type_expr()?);
                    if !self.eat(TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(TokenKind::RParen)?;
                let ret = self.type_expr()?;
                Some(TypeExpr::Fn {
                    params,
                    span: start.to(ret.span()),
                    ret: Box::new(ret),
                })
            }
            TokenKind::Ident(_) => {
                let mut segments = vec![self.ident()?];
                // `http.Status4xx` -- a type reached through a module.
                while self.eat(TokenKind::Dot) {
                    segments.push(self.ident()?);
                }
                // `Box[i64]`. Unambiguous: indexing is an expression, and no
                // expression can appear where a type is expected.
                let mut args = Vec::new();
                if self.eat(TokenKind::LBracket) {
                    while !self.at(&TokenKind::RBracket) && !self.at_eof() {
                        args.push(self.type_expr()?);
                        if !self.eat(TokenKind::Comma) {
                            break;
                        }
                    }
                    self.expect(TokenKind::RBracket)?;
                }
                if segments.len() == 1 && args.is_empty() {
                    return Some(TypeExpr::Named(segments.pop().expect("one segment")));
                }
                Some(TypeExpr::Path {
                    span: segments[0].span.to(self.prev_span()),
                    segments,
                    args,
                })
            }
            other => {
                let found = other.describe();
                self.error(start, format!("expected a type, found {found}"));
                None
            }
        }
    }

    // ---- statements -----------------------------------------------------

    fn block(&mut self) -> Option<Block> {
        self.scoped(|p| {
            p.enter("block")?;
            p.block_inner()
        })
    }

    fn block_inner(&mut self) -> Option<Block> {
        let start = self.span();
        self.expect(TokenKind::LBrace)?;
        let mut stmts = Vec::new();
        while !self.at(&TokenKind::RBrace) && !self.at_eof() {
            let before = self.pos;
            match self.stmt() {
                Some(s) => stmts.push(s),
                None => self.recover_to_stmt(),
            }
            if self.pos == before {
                self.bump();
            }
        }
        self.expect(TokenKind::RBrace)?;
        Some(Block {
            stmts,
            span: start.to(self.prev_span()),
        })
    }

    fn stmt(&mut self) -> Option<Stmt> {
        let start = self.span();
        match self.peek() {
            TokenKind::Const | TokenKind::Var => {
                let mutable = matches!(self.peek(), TokenKind::Var);
                self.bump();
                let name = self.ident()?;
                let ty = if self.eat(TokenKind::Colon) {
                    Some(self.type_expr()?)
                } else {
                    None
                };
                self.expect(TokenKind::Eq)?;
                let init = self.expr()?;
                self.expect(TokenKind::Semi)?;
                Some(Stmt::Let(LetStmt {
                    mutable,
                    name,
                    ty,
                    init,
                    span: start.to(self.prev_span()),
                }))
            }
            TokenKind::Return => {
                self.bump();
                let value = if self.at(&TokenKind::Semi) {
                    None
                } else {
                    Some(self.expr()?)
                };
                self.expect(TokenKind::Semi)?;
                Some(Stmt::Return {
                    value,
                    span: start.to(self.prev_span()),
                })
            }
            TokenKind::Break => {
                self.bump();
                self.expect(TokenKind::Semi)?;
                Some(Stmt::Break(start.to(self.prev_span())))
            }
            TokenKind::Continue => {
                self.bump();
                self.expect(TokenKind::Semi)?;
                Some(Stmt::Continue(start.to(self.prev_span())))
            }
            TokenKind::If => self.if_stmt_or_expr(),
            TokenKind::While => self.while_stmt().map(Stmt::While),
            TokenKind::For => self.for_stmt().map(Stmt::For),
            TokenKind::LBrace => self.block().map(Stmt::Block),
            _ => self.expr_or_assign_stmt(),
        }
    }

    fn expr_or_assign_stmt(&mut self) -> Option<Stmt> {
        let start = self.span();
        let target = self.expr()?;
        let op = match self.peek() {
            TokenKind::Eq => None,
            TokenKind::PlusEq => Some(BinOp::Add),
            TokenKind::MinusEq => Some(BinOp::Sub),
            TokenKind::StarEq => Some(BinOp::Mul),
            TokenKind::SlashEq => Some(BinOp::Div),
            TokenKind::PercentEq => Some(BinOp::Rem),
            _ => {
                self.expect(TokenKind::Semi)?;
                return Some(Stmt::Expr(target));
            }
        };
        self.bump();

        if !matches!(
            target,
            Expr::Ident(_) | Expr::Field { .. } | Expr::Index { .. }
        ) {
            self.error(target.span(), "cannot assign to this expression")
        }
        let value = self.expr()?;
        self.expect(TokenKind::Semi)?;
        Some(Stmt::Assign(AssignStmt {
            target,
            op,
            value,
            span: start.to(self.prev_span()),
        }))
    }

    /// `if` in statement position. The braces decide the form: `if (c) { .. }`
    /// is a statement, `if (c) a else b;` is an expression statement.
    fn if_stmt_or_expr(&mut self) -> Option<Stmt> {
        let start = self.span();
        self.expect(TokenKind::If)?;
        self.expect(TokenKind::LParen)?;
        let cond = self.expr()?;
        self.expect(TokenKind::RParen)?;
        let capture = self.opt_capture()?;

        if !self.at(&TokenKind::LBrace) {
            let expr = self.if_expr_tail(start, cond, capture)?;
            self.expect(TokenKind::Semi)?;
            return Some(Stmt::Expr(expr));
        }

        let then = self.block()?;
        let else_ = if self.eat(TokenKind::Else) {
            if self.at(&TokenKind::If) {
                // `else if` -- recurse, requiring the statement form. A chain
                // of them nests in the tree (and in every later pass), so it
                // counts against the depth limit like any other nesting.
                let inner = self.scoped(|p| {
                    p.enter("`else if` chain")?;
                    p.if_stmt_or_expr()
                });
                match inner? {
                    Stmt::If(inner) => Some(Box::new(ElseBranch::If(inner))),
                    other => {
                        self.error(other.span(), "`else if` must use a block body");
                        None
                    }
                }
            } else {
                Some(Box::new(ElseBranch::Block(self.block()?)))
            }
        } else {
            None
        };
        Some(Stmt::If(IfStmt {
            cond,
            capture,
            then,
            else_,
            span: start.to(self.prev_span()),
        }))
    }

    fn while_stmt(&mut self) -> Option<WhileStmt> {
        let start = self.span();
        self.expect(TokenKind::While)?;
        self.expect(TokenKind::LParen)?;
        let cond = self.expr()?;
        self.expect(TokenKind::RParen)?;
        let capture = self.opt_capture()?;

        // `: (i += 1)` -- the continue expression.
        let cont = if self.eat(TokenKind::Colon) {
            self.expect(TokenKind::LParen)?;
            let inner_start = self.span();
            let target = self.expr()?;
            let stmt = match self.peek() {
                TokenKind::Eq => Some(None),
                TokenKind::PlusEq => Some(Some(BinOp::Add)),
                TokenKind::MinusEq => Some(Some(BinOp::Sub)),
                TokenKind::StarEq => Some(Some(BinOp::Mul)),
                TokenKind::SlashEq => Some(Some(BinOp::Div)),
                TokenKind::PercentEq => Some(Some(BinOp::Rem)),
                _ => None,
            };
            let stmt = match stmt {
                Some(op) => {
                    self.bump();
                    let value = self.expr()?;
                    Stmt::Assign(AssignStmt {
                        target,
                        op,
                        value,
                        span: inner_start.to(self.prev_span()),
                    })
                }
                None => Stmt::Expr(target),
            };
            self.expect(TokenKind::RParen)?;
            Some(Box::new(stmt))
        } else {
            None
        };

        let body = self.block()?;
        Some(WhileStmt {
            cond,
            capture,
            cont,
            body,
            span: start.to(self.prev_span()),
        })
    }

    /// An optional `|name|` payload capture.
    /// `for (xs) |x| { }`, or `for (xs) |x, i| { }` to bind the index too.
    ///
    /// The capture is required: a `for` with nothing bound would be a `while`
    /// with extra steps.
    fn for_stmt(&mut self) -> Option<ForStmt> {
        let start = self.span();
        self.expect(TokenKind::For)?;
        self.expect(TokenKind::LParen)?;
        let iter = self.expr()?;
        self.expect(TokenKind::RParen)?;

        let cap_start = self.span();
        if !self.eat(TokenKind::Pipe) {
            self.error_with_help(
                cap_start,
                "expected `|` after the value being iterated",
                "a `for` binds each element, as in `for (xs) |x| { .. }`",
            );
            return None;
        }
        let value = self.ident()?;
        let index = if self.eat(TokenKind::Comma) {
            Some(self.ident()?)
        } else {
            None
        };
        self.expect(TokenKind::Pipe)?;

        let body = self.block()?;
        Some(ForStmt {
            iter,
            value,
            index,
            body,
            span: start.to(self.prev_span()),
        })
    }

    fn opt_capture(&mut self) -> Option<Option<Ident>> {
        if !self.eat(TokenKind::Pipe) {
            return Some(None);
        }
        let name = self.ident()?;
        self.expect(TokenKind::Pipe)?;
        Some(Some(name))
    }

    // ---- expressions ----------------------------------------------------

    fn expr(&mut self) -> Option<Expr> {
        self.scoped(|p| {
            p.enter("expression")?;
            p.or_expr()
        })
    }

    fn or_expr(&mut self) -> Option<Expr> {
        self.scoped(|p| {
            let mut lhs = p.and_expr()?;
            while p.at(&TokenKind::Or) {
                p.enter("expression")?;
                p.bump();
                let rhs = p.and_expr()?;
                let span = lhs.span().to(rhs.span());
                lhs = Expr::Binary {
                    op: BinOp::Or,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                    span,
                };
            }
            Some(lhs)
        })
    }

    fn and_expr(&mut self) -> Option<Expr> {
        self.scoped(|p| {
            let mut lhs = p.cmp_expr()?;
            while p.at(&TokenKind::And) {
                p.enter("expression")?;
                p.bump();
                let rhs = p.cmp_expr()?;
                let span = lhs.span().to(rhs.span());
                lhs = Expr::Binary {
                    op: BinOp::And,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                    span,
                };
            }
            Some(lhs)
        })
    }

    /// Comparisons are non-associative, so `a < b < c` is rejected with a hint
    /// rather than silently parsed as `(a < b) < c`.
    fn cmp_expr(&mut self) -> Option<Expr> {
        self.scoped(|p| {
            let lhs = p.catch_expr()?;
            let Some(op) = cmp_op(p.peek()) else {
                return Some(lhs);
            };
            p.enter("expression")?;
            p.bump();
            let rhs = p.catch_expr()?;
            let span = lhs.span().to(rhs.span());
            let result = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                span,
            };

            if let Some(second) = cmp_op(p.peek()) {
                let at = p.span();
                p.error_with_help(
                    at,
                    "comparison operators cannot be chained",
                    format!(
                        "split the comparison, e.g. `a {} b and b {} c`",
                        op.text(),
                        second.text()
                    ),
                );
                return None;
            }
            Some(result)
        })
    }

    /// `orelse` and `catch`, which bind tighter than comparison (as in Zig).
    fn catch_expr(&mut self) -> Option<Expr> {
        self.scoped(|p| {
            let mut lhs = p.add_expr()?;
            loop {
                match p.peek() {
                    TokenKind::Orelse => {
                        p.enter("expression")?;
                        p.bump();
                        let alt = p.add_expr()?;
                        let span = lhs.span().to(alt.span());
                        lhs = Expr::Orelse {
                            expr: Box::new(lhs),
                            alt: Box::new(alt),
                            span,
                        };
                    }
                    TokenKind::Catch => {
                        p.enter("expression")?;
                        p.bump();
                        let capture = p.opt_capture()?;
                        let alt = p.add_expr()?;
                        let span = lhs.span().to(alt.span());
                        lhs = Expr::Catch {
                            expr: Box::new(lhs),
                            capture,
                            alt: Box::new(alt),
                            span,
                        };
                    }
                    _ => return Some(lhs),
                }
            }
        })
    }

    fn add_expr(&mut self) -> Option<Expr> {
        self.scoped(|p| {
            let mut lhs = p.mul_expr()?;
            loop {
                let op = match p.peek() {
                    TokenKind::Plus => BinOp::Add,
                    TokenKind::Minus => BinOp::Sub,
                    _ => return Some(lhs),
                };
                p.enter("expression")?;
                p.bump();
                let rhs = p.mul_expr()?;
                let span = lhs.span().to(rhs.span());
                lhs = Expr::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                    span,
                };
            }
        })
    }

    fn mul_expr(&mut self) -> Option<Expr> {
        self.scoped(|p| {
            let mut lhs = p.unary_expr()?;
            loop {
                let op = match p.peek() {
                    TokenKind::Star => BinOp::Mul,
                    TokenKind::Slash => BinOp::Div,
                    TokenKind::Percent => BinOp::Rem,
                    _ => return Some(lhs),
                };
                p.enter("expression")?;
                p.bump();
                let rhs = p.unary_expr()?;
                let span = lhs.span().to(rhs.span());
                lhs = Expr::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                    span,
                };
            }
        })
    }

    fn unary_expr(&mut self) -> Option<Expr> {
        let start = self.span();
        match self.peek() {
            TokenKind::Minus => {
                self.bump();
                let expr = self.unary_operand()?;
                Some(Expr::Unary {
                    op: UnOp::Neg,
                    span: start.to(expr.span()),
                    expr: Box::new(expr),
                })
            }
            TokenKind::Bang => {
                self.bump();
                let expr = self.unary_operand()?;
                Some(Expr::Unary {
                    op: UnOp::Not,
                    span: start.to(expr.span()),
                    expr: Box::new(expr),
                })
            }
            TokenKind::Try => {
                self.bump();
                let expr = self.unary_operand()?;
                Some(Expr::Try {
                    span: start.to(expr.span()),
                    expr: Box::new(expr),
                })
            }
            _ => self.postfix_expr(),
        }
    }

    /// The operand of a prefix operator. Prefix operators recurse directly
    /// into `unary_expr` without passing through `expr`, so they need their
    /// own depth accounting: `----...-1` is otherwise unbounded.
    fn unary_operand(&mut self) -> Option<Expr> {
        self.scoped(|p| {
            p.enter("expression")?;
            p.unary_expr()
        })
    }

    fn postfix_expr(&mut self) -> Option<Expr> {
        self.scoped(|p| {
            let mut expr = p.primary_expr()?;
            loop {
                match p.peek() {
                    TokenKind::LParen => {
                        p.enter("expression")?;
                        p.bump();
                        let mut args = Vec::new();
                        while !p.at(&TokenKind::RParen) && !p.at_eof() {
                            args.push(p.expr()?);
                            if !p.eat(TokenKind::Comma) {
                                break;
                            }
                        }
                        p.expect(TokenKind::RParen)?;
                        let span = expr.span().to(p.prev_span());
                        expr = Expr::Call {
                            callee: Box::new(expr),
                            args,
                            span,
                        };
                    }
                    TokenKind::Dot => {
                        p.enter("expression")?;
                        p.bump();
                        let name = p.ident()?;
                        // `util.Point{ .x = 1 }` -- a struct named through a
                        // module. Unambiguous for the same reason a bare name
                        // is: nothing that takes a block puts a path before
                        // its `{`.
                        if p.at(&TokenKind::LBrace)
                            && let Some(mut path) = path_of(&expr)
                        {
                            path.push(name);
                            return p.struct_lit(path);
                        }
                        let span = expr.span().to(name.span);
                        expr = Expr::Field {
                            obj: Box::new(expr),
                            name,
                            span,
                        };
                    }
                    TokenKind::LBracket => {
                        p.enter("expression")?;
                        p.bump();
                        let index = p.expr()?;
                        p.expect(TokenKind::RBracket)?;
                        let span = expr.span().to(p.prev_span());
                        expr = Expr::Index {
                            obj: Box::new(expr),
                            index: Box::new(index),
                            span,
                        };
                    }
                    TokenKind::DotQuestion => {
                        p.enter("expression")?;
                        p.bump();
                        let span = expr.span().to(p.prev_span());
                        expr = Expr::Unwrap {
                            expr: Box::new(expr),
                            span,
                        };
                    }
                    _ => return Some(expr),
                }
            }
        })
    }

    fn primary_expr(&mut self) -> Option<Expr> {
        let span = self.span();
        match self.peek().clone() {
            // `[]i64{ 1, 2, 3 }`. Unambiguous with the postfix `a[i]`, which
            // only ever follows an expression, and with `Point{ .x = 1 }`,
            // whose `{` follows a bare identifier.
            TokenKind::LBracket => self.array_lit(span),
            TokenKind::At => self.builtin_form(span),
            TokenKind::Int(v) => {
                self.bump();
                Some(Expr::Int(v, span))
            }
            TokenKind::Float(v) => {
                self.bump();
                Some(Expr::Float(v, span))
            }
            TokenKind::Str(v) => {
                self.bump();
                Some(Expr::Str(v, span))
            }
            TokenKind::True => {
                self.bump();
                Some(Expr::Bool(true, span))
            }
            TokenKind::False => {
                self.bump();
                Some(Expr::Bool(false, span))
            }
            TokenKind::Null => {
                self.bump();
                Some(Expr::Null(span))
            }
            TokenKind::Error => {
                self.bump();
                self.expect(TokenKind::Dot)?;
                let name = self.ident()?;
                Some(Expr::ErrorLit {
                    span: span.to(name.span),
                    name,
                })
            }
            TokenKind::LParen => {
                self.bump();
                let inner = self.expr()?;
                self.expect(TokenKind::RParen)?;
                Some(inner)
            }
            TokenKind::Fn => {
                self.bump();
                // `fn [T](..)` names type parameters exactly as a declaration
                // does; a `const` bound to such a literal is a definition, and
                // generalises.
                let generics = self.generic_params()?;
                let func = self.func_rest(span, generics)?;
                Some(Expr::Fn(Box::new(func)))
            }
            TokenKind::If => {
                self.bump();
                self.expect(TokenKind::LParen)?;
                let cond = self.expr()?;
                self.expect(TokenKind::RParen)?;
                let capture = self.opt_capture()?;
                self.if_expr_tail(span, cond, capture)
            }
            TokenKind::Ident(name) => {
                self.bump();
                let ident = Ident::new(name, span);
                // `Point{ .x = 1 }`. Unambiguous because every construct that
                // takes a block (`if`, `while`, `fn`) puts a `)` or a type
                // before its `{`, never a bare identifier.
                if self.at(&TokenKind::LBrace) {
                    return self.struct_lit(vec![ident]);
                }
                Some(Expr::Ident(ident))
            }
            other => {
                let found = other.describe();
                self.error(span, format!("expected an expression, found {found}"));
                None
            }
        }
    }

    /// The `a else b` tail of an `if` expression, after the condition.
    fn if_expr_tail(&mut self, start: Span, cond: Expr, capture: Option<Ident>) -> Option<Expr> {
        let then = self.expr()?;
        if !self.at(&TokenKind::Else) {
            self.error_with_help(
                self.span(),
                "expected `else`",
                "an `if` used as an expression must have an `else`, because it always produces a value",
            );
            return None;
        }
        self.bump();
        let else_ = self.expr()?;
        let span = start.to(else_.span());
        Some(Expr::If(Box::new(IfExpr {
            cond,
            capture,
            then,
            else_,
            span,
        })))
    }

    /// `@import("std/http")`.
    ///
    /// `@name(..)` is Zig's spelling for a form the compiler handles rather
    /// than a function it could call. `import` is the only one so far, and it
    /// has to be one: its argument names a file to read, which is a question
    /// asked long before anything runs.
    fn builtin_form(&mut self, start: Span) -> Option<Expr> {
        self.expect(TokenKind::At)?;
        let name = self.ident()?;
        if name.as_str() != "import" {
            self.error_with_help(
                name.span,
                format!("unknown builtin `@{name}`"),
                "the only one is `@import(\"path\")`",
            );
            return None;
        }
        self.expect(TokenKind::LParen)?;
        let path_span = self.span();
        let TokenKind::Str(path) = self.peek().clone() else {
            let found = self.peek().describe();
            self.error_with_help(
                path_span,
                format!("expected a module path, found {found}"),
                "`@import` takes a string, as in `@import(\"std/http\")`",
            );
            return None;
        };
        self.bump();
        self.expect(TokenKind::RParen)?;
        Some(Expr::Import {
            path,
            span: start.to(self.prev_span()),
        })
    }

    /// `[]T{ a, b }` -- the element type, then the elements.
    ///
    /// The type is written rather than inferred from the elements so that an
    /// empty literal still has one: `[]i64{}` is a value, and nothing else in
    /// the expression would say what it holds.
    fn array_lit(&mut self, start: Span) -> Option<Expr> {
        self.scoped(|p| {
            p.enter("expression")?;
            let TypeExpr::Array { elem, .. } = p.type_expr_inner()? else {
                unreachable!("the caller saw `[`")
            };
            p.expect(TokenKind::LBrace)?;
            let mut elems = Vec::new();
            while !p.at(&TokenKind::RBrace) && !p.at_eof() {
                elems.push(p.expr()?);
                if !p.eat(TokenKind::Comma) {
                    break;
                }
            }
            p.expect(TokenKind::RBrace)?;
            Some(Expr::ArrayLit {
                elem: *elem,
                elems,
                span: start.to(p.prev_span()),
            })
        })
    }

    fn struct_lit(&mut self, path: Vec<Ident>) -> Option<Expr> {
        self.expect(TokenKind::LBrace)?;
        let mut fields = Vec::new();
        while !self.at(&TokenKind::RBrace) && !self.at_eof() {
            let f_start = self.span();
            self.expect(TokenKind::Dot)?;
            let f_name = self.ident()?;
            self.expect(TokenKind::Eq)?;
            let value = self.expr()?;
            fields.push(FieldInit {
                name: f_name,
                value,
                span: f_start.to(self.prev_span()),
            });
            if !self.eat(TokenKind::Comma) {
                break;
            }
        }
        self.expect(TokenKind::RBrace)?;
        let span = path[0].span.to(self.prev_span());
        Some(Expr::StructLit { path, fields, span })
    }
}

/// The identifiers making up a dotted path, if that is all the expression is.
fn path_of(expr: &Expr) -> Option<Vec<Ident>> {
    match expr {
        Expr::Ident(name) => Some(vec![name.clone()]),
        Expr::Field { obj, name, .. } => {
            let mut path = path_of(obj)?;
            path.push(name.clone());
            Some(path)
        }
        _ => None,
    }
}

fn cmp_op(kind: &TokenKind) -> Option<BinOp> {
    Some(match kind {
        TokenKind::EqEq => BinOp::Eq,
        TokenKind::BangEq => BinOp::Ne,
        TokenKind::Lt => BinOp::Lt,
        TokenKind::LtEq => BinOp::Le,
        TokenKind::Gt => BinOp::Gt,
        TokenKind::GtEq => BinOp::Ge,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parser_for(src: &str) -> Parser {
        let (tokens, _) = crate::lexer::lex(src);
        Parser {
            tokens,
            pos: 0,
            diags: Vec::new(),
            depth: 0,
            too_deep: false,
        }
    }

    #[test]
    fn help_stays_with_the_diagnostic_it_was_written_for() {
        // Two errors at one position: the second is an echo and is dropped,
        // and its help must be dropped with it rather than attached to the
        // first, which is about something else.
        let mut p = parser_for("x");
        let span = p.span();
        p.error(span, "first");
        p.error_with_help(span, "second", "help for the second");
        assert_eq!(p.diags.len(), 1);
        assert_eq!(p.diags[0].message, "first");
        assert_eq!(p.diags[0].help, None);

        // When it is not an echo, the help lands on its own diagnostic.
        let mut p = parser_for("x");
        let span = p.span();
        p.error_with_help(span, "only", "its help");
        assert_eq!(p.diags[0].help.as_deref(), Some("its help"));
    }

    /// Messages of the diagnostics from parsing `src`.
    ///
    /// Runs on a thread with a generous stack: the limit is sized for the
    /// compiler's main thread, and a debug-build parser at the limit does not
    /// fit in the test harness's 2 MiB threads.
    fn messages(src: String) -> Vec<String> {
        std::thread::Builder::new()
            .stack_size(64 << 20)
            .spawn(move || {
                let (_, diags) = parse(&src);
                diags.into_iter().map(|d| d.message).collect()
            })
            .expect("spawn")
            .join()
            .expect("parse thread panicked")
    }

    #[test]
    fn nesting_past_the_limit_is_reported_once() {
        // Prefix operators recurse without going through `expr`.
        let src = format!("fn main() i64 {{ return {}1; }}", "-".repeat(1000));
        let (_, diags) = parse(&src);
        let deep: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("nested too deeply"))
            .collect();
        assert_eq!(
            deep.len(),
            1,
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
        assert_eq!(
            deep[0].help.as_deref(),
            Some(format!("the limit is {MAX_DEPTH} levels").as_str())
        );
    }

    #[test]
    fn every_kind_of_nesting_is_bounded() {
        // Each of these recurses (or accumulates a left-nested tree, which
        // every later pass recurses on) through a different path.
        let cases = [
            (
                format!(
                    "fn main() i64 {{ return {}1{}; }}",
                    "(".repeat(1000),
                    ")".repeat(1000)
                ),
                "expression is nested too deeply",
            ),
            (
                format!("fn main() i64 {{ return {}1; }}", "1 + ".repeat(1000)),
                "expression is nested too deeply",
            ),
            (
                format!("fn main() i64 {{ return f{}; }}", "()".repeat(1000)),
                "expression is nested too deeply",
            ),
            (
                format!("fn main() i64 {{ return x{}; }}", ".f".repeat(1000)),
                "expression is nested too deeply",
            ),
            (
                format!("fn f(x: {}i64) void {{ }}", "?".repeat(1000)),
                "type is nested too deeply",
            ),
            (
                format!("fn f(x: {}i64) void {{ }}", "[]".repeat(1000)),
                "type is nested too deeply",
            ),
            (
                format!("fn main() i64 {{ return x{}; }}", "[0]".repeat(1000)),
                "expression is nested too deeply",
            ),
            (
                format!("fn main() void {}{}", "{".repeat(1000), "}".repeat(1000)),
                "block is nested too deeply",
            ),
            (
                format!(
                    "fn main() void {{ if (true) {{ }} {}}}",
                    "else if (true) { } ".repeat(1000)
                ),
                "nested too deeply",
            ),
        ];
        for (src, expected) in cases {
            let got = messages(src);
            assert!(
                got.iter().any(|m| m.contains(expected)),
                "expected {expected:?}, got {got:?}"
            );
        }
    }

    #[test]
    fn nesting_within_the_limit_is_fine() {
        let n = (MAX_DEPTH / 2) as usize;
        let src = format!(
            "fn main() i64 {{ return {}1{}; }}",
            "(".repeat(n),
            ")".repeat(n)
        );
        assert!(messages(src).is_empty());
        let src = format!("fn main() i64 {{ return {}1; }}", "1 + ".repeat(n));
        assert!(messages(src).is_empty());
    }
}
