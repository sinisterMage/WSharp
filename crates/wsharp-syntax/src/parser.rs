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
use crate::lexer::lex;
use crate::span::{Ident, Span};
use crate::token::{Token, TokenKind};

/// Parse a source file. The module is returned even when there were errors (it
/// simply omits the parts that could not be parsed), so later passes can still
/// run and report more.
pub fn parse(src: &str) -> (Module, Vec<Diagnostic>) {
    let (tokens, mut diags) = lex(src);
    let mut parser = Parser {
        tokens,
        pos: 0,
        diags: Vec::new(),
        fuel: 0,
    };
    let module = parser.module();
    diags.append(&mut parser.diags);
    (module, diags)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    diags: Vec<Diagnostic>,
    /// Guards against a recovery loop that fails to consume anything.
    fuel: u32,
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
        // Only the first error at a given position is useful; the rest are echoes.
        if self.diags.last().is_some_and(|d| d.primary.span == span) {
            return;
        }
        self.diags.push(Diagnostic::error(span, message));
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
                self.error(span, "`var` is not allowed at the top level");
                self.diags.last_mut().unwrap().help =
                    Some("top-level bindings must be `const`".into());
                None
            }
            other => {
                let found = other.describe();
                self.error(
                    self.span(),
                    format!("expected a declaration, found {found}"),
                );
                self.diags.last_mut().unwrap().help =
                    Some("a W# file contains `fn` and `const` declarations".into());
                None
            }
        }
    }

    fn fn_decl(&mut self) -> Option<FnDecl> {
        let start = self.span();
        self.expect(TokenKind::Fn)?;
        let name = self.ident()?;
        let func = self.func_rest(start)?;
        let span = start.to(func.span);
        Some(FnDecl { name, func, span })
    }

    /// The `(params) RetType { body }` tail shared by `fn` declarations and
    /// `fn` literals. `start` is the span of the `fn` keyword.
    fn func_rest(&mut self, start: Span) -> Option<Func> {
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
            // `struct : Parent { ... }` declares a subtype. The colon is
            // unambiguous here: the annotation slot before `=` was already
            // consumed above, and a struct body always starts with `{`.
            let parent = if self.eat(TokenKind::Colon) {
                Some(self.ident()?)
            } else {
                None
            };
            let fields = self.struct_body()?;
            self.expect(TokenKind::Semi)?;
            let span = start.to(self.prev_span());
            return Some(Item::Struct(StructDecl {
                name,
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
        let start = self.span();
        match self.peek() {
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
            TokenKind::Ident(_) => Some(TypeExpr::Named(self.ident()?)),
            other => {
                let found = other.describe();
                self.error(start, format!("expected a type, found {found}"));
                None
            }
        }
    }

    // ---- statements -----------------------------------------------------

    fn block(&mut self) -> Option<Block> {
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
        let op_span = self.span();
        self.bump();

        if !matches!(target, Expr::Ident(_) | Expr::Field { .. }) {
            self.error(target.span(), "cannot assign to this expression")
        }
        let value = self.expr()?;
        self.expect(TokenKind::Semi)?;
        let _ = op_span;
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
                // `else if` -- recurse, requiring the statement form.
                match self.if_stmt_or_expr()? {
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
        self.fuel += 1;
        if self.fuel > 100_000 {
            return None;
        }
        let e = self.or_expr();
        self.fuel -= 1;
        e
    }

    fn or_expr(&mut self) -> Option<Expr> {
        let mut lhs = self.and_expr()?;
        while self.at(&TokenKind::Or) {
            self.bump();
            let rhs = self.and_expr()?;
            let span = lhs.span().to(rhs.span());
            lhs = Expr::Binary {
                op: BinOp::Or,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                span,
            };
        }
        Some(lhs)
    }

    fn and_expr(&mut self) -> Option<Expr> {
        let mut lhs = self.cmp_expr()?;
        while self.at(&TokenKind::And) {
            self.bump();
            let rhs = self.cmp_expr()?;
            let span = lhs.span().to(rhs.span());
            lhs = Expr::Binary {
                op: BinOp::And,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                span,
            };
        }
        Some(lhs)
    }

    /// Comparisons are non-associative, so `a < b < c` is rejected with a hint
    /// rather than silently parsed as `(a < b) < c`.
    fn cmp_expr(&mut self) -> Option<Expr> {
        let lhs = self.catch_expr()?;
        let Some(op) = cmp_op(self.peek()) else {
            return Some(lhs);
        };
        self.bump();
        let rhs = self.catch_expr()?;
        let span = lhs.span().to(rhs.span());
        let result = Expr::Binary {
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
            span,
        };

        if let Some(second) = cmp_op(self.peek()) {
            let at = self.span();
            self.error(at, "comparison operators cannot be chained");
            self.diags.last_mut().unwrap().help = Some(format!(
                "split the comparison, e.g. `a {} b and b {} c`",
                op.text(),
                second.text()
            ));
            return None;
        }
        Some(result)
    }

    /// `orelse` and `catch`, which bind tighter than comparison (as in Zig).
    fn catch_expr(&mut self) -> Option<Expr> {
        let mut lhs = self.add_expr()?;
        loop {
            match self.peek() {
                TokenKind::Orelse => {
                    self.bump();
                    let alt = self.add_expr()?;
                    let span = lhs.span().to(alt.span());
                    lhs = Expr::Orelse {
                        expr: Box::new(lhs),
                        alt: Box::new(alt),
                        span,
                    };
                }
                TokenKind::Catch => {
                    self.bump();
                    let capture = self.opt_capture()?;
                    let alt = self.add_expr()?;
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
    }

    fn add_expr(&mut self) -> Option<Expr> {
        let mut lhs = self.mul_expr()?;
        loop {
            let op = match self.peek() {
                TokenKind::Plus => BinOp::Add,
                TokenKind::Minus => BinOp::Sub,
                _ => return Some(lhs),
            };
            self.bump();
            let rhs = self.mul_expr()?;
            let span = lhs.span().to(rhs.span());
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                span,
            };
        }
    }

    fn mul_expr(&mut self) -> Option<Expr> {
        let mut lhs = self.unary_expr()?;
        loop {
            let op = match self.peek() {
                TokenKind::Star => BinOp::Mul,
                TokenKind::Slash => BinOp::Div,
                TokenKind::Percent => BinOp::Rem,
                _ => return Some(lhs),
            };
            self.bump();
            let rhs = self.unary_expr()?;
            let span = lhs.span().to(rhs.span());
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                span,
            };
        }
    }

    fn unary_expr(&mut self) -> Option<Expr> {
        let start = self.span();
        match self.peek() {
            TokenKind::Minus => {
                self.bump();
                let expr = self.unary_expr()?;
                Some(Expr::Unary {
                    op: UnOp::Neg,
                    span: start.to(expr.span()),
                    expr: Box::new(expr),
                })
            }
            TokenKind::Bang => {
                self.bump();
                let expr = self.unary_expr()?;
                Some(Expr::Unary {
                    op: UnOp::Not,
                    span: start.to(expr.span()),
                    expr: Box::new(expr),
                })
            }
            TokenKind::Try => {
                self.bump();
                let expr = self.unary_expr()?;
                Some(Expr::Try {
                    span: start.to(expr.span()),
                    expr: Box::new(expr),
                })
            }
            _ => self.postfix_expr(),
        }
    }

    fn postfix_expr(&mut self) -> Option<Expr> {
        let mut expr = self.primary_expr()?;
        loop {
            match self.peek() {
                TokenKind::LParen => {
                    self.bump();
                    let mut args = Vec::new();
                    while !self.at(&TokenKind::RParen) && !self.at_eof() {
                        args.push(self.expr()?);
                        if !self.eat(TokenKind::Comma) {
                            break;
                        }
                    }
                    self.expect(TokenKind::RParen)?;
                    let span = expr.span().to(self.prev_span());
                    expr = Expr::Call {
                        callee: Box::new(expr),
                        args,
                        span,
                    };
                }
                TokenKind::Dot => {
                    self.bump();
                    let name = self.ident()?;
                    let span = expr.span().to(name.span);
                    expr = Expr::Field {
                        obj: Box::new(expr),
                        name,
                        span,
                    };
                }
                TokenKind::DotQuestion => {
                    self.bump();
                    let span = expr.span().to(self.prev_span());
                    expr = Expr::Unwrap {
                        expr: Box::new(expr),
                        span,
                    };
                }
                _ => return Some(expr),
            }
        }
    }

    fn primary_expr(&mut self) -> Option<Expr> {
        let span = self.span();
        match self.peek().clone() {
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
                let func = self.func_rest(span)?;
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
                    return self.struct_lit(ident);
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
            self.error(self.span(), "expected `else`");
            self.diags.last_mut().unwrap().help = Some(
                "an `if` used as an expression must have an `else`, because it always produces a value"
                    .into(),
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

    fn struct_lit(&mut self, name: Ident) -> Option<Expr> {
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
        let span = name.span.to(self.prev_span());
        Some(Expr::StructLit { name, fields, span })
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
