//! Token kinds produced by the lexer.

use crate::span::Span;

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // Literals.
    Int(i64),
    Float(f64),
    Str(Box<str>),
    Ident(Box<str>),

    // Keywords. Primitive type names (`i64`, `bool`, `str`, ...) are *not*
    // keywords -- they are ordinary identifiers resolved in the type namespace.
    And,
    Break,
    Catch,
    Const,
    Continue,
    Else,
    Error,
    False,
    Fn,
    If,
    Null,
    Or,
    Orelse,
    Return,
    Struct,
    True,
    Try,
    Var,
    While,

    // Delimiters.
    LParen,
    RParen,
    LBrace,
    RBrace,

    // Punctuation.
    Comma,
    Semi,
    Colon,
    Dot,
    DotQuestion,
    Pipe,
    Question,

    // Operators.
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Bang,
    Eq,
    EqEq,
    BangEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,

    Eof,
}

impl TokenKind {
    /// How this token is named in "expected X, found Y" messages.
    pub fn describe(&self) -> String {
        match self {
            TokenKind::Int(_) => "an integer literal".into(),
            TokenKind::Float(_) => "a float literal".into(),
            TokenKind::Str(_) => "a string literal".into(),
            TokenKind::Ident(n) => format!("`{n}`"),
            TokenKind::Eof => "end of file".into(),
            other => format!("`{}`", other.text()),
        }
    }

    /// The literal source text for fixed tokens. Panics for value-carrying ones.
    pub fn text(&self) -> &'static str {
        match self {
            TokenKind::And => "and",
            TokenKind::Break => "break",
            TokenKind::Catch => "catch",
            TokenKind::Const => "const",
            TokenKind::Continue => "continue",
            TokenKind::Else => "else",
            TokenKind::Error => "error",
            TokenKind::False => "false",
            TokenKind::Fn => "fn",
            TokenKind::If => "if",
            TokenKind::Null => "null",
            TokenKind::Or => "or",
            TokenKind::Orelse => "orelse",
            TokenKind::Return => "return",
            TokenKind::Struct => "struct",
            TokenKind::True => "true",
            TokenKind::Try => "try",
            TokenKind::Var => "var",
            TokenKind::While => "while",
            TokenKind::LParen => "(",
            TokenKind::RParen => ")",
            TokenKind::LBrace => "{",
            TokenKind::RBrace => "}",
            TokenKind::Comma => ",",
            TokenKind::Semi => ";",
            TokenKind::Colon => ":",
            TokenKind::Dot => ".",
            TokenKind::DotQuestion => ".?",
            TokenKind::Pipe => "|",
            TokenKind::Question => "?",
            TokenKind::Plus => "+",
            TokenKind::Minus => "-",
            TokenKind::Star => "*",
            TokenKind::Slash => "/",
            TokenKind::Percent => "%",
            TokenKind::Bang => "!",
            TokenKind::Eq => "=",
            TokenKind::EqEq => "==",
            TokenKind::BangEq => "!=",
            TokenKind::Lt => "<",
            TokenKind::LtEq => "<=",
            TokenKind::Gt => ">",
            TokenKind::GtEq => ">=",
            TokenKind::PlusEq => "+=",
            TokenKind::MinusEq => "-=",
            TokenKind::StarEq => "*=",
            TokenKind::SlashEq => "/=",
            TokenKind::PercentEq => "%=",
            TokenKind::Eof => "<eof>",
            TokenKind::Int(_) | TokenKind::Float(_) | TokenKind::Str(_) | TokenKind::Ident(_) => {
                unreachable!("value-carrying token has no fixed text")
            }
        }
    }
}

pub fn keyword(word: &str) -> Option<TokenKind> {
    Some(match word {
        "and" => TokenKind::And,
        "break" => TokenKind::Break,
        "catch" => TokenKind::Catch,
        "const" => TokenKind::Const,
        "continue" => TokenKind::Continue,
        "else" => TokenKind::Else,
        "error" => TokenKind::Error,
        "false" => TokenKind::False,
        "fn" => TokenKind::Fn,
        "if" => TokenKind::If,
        "null" => TokenKind::Null,
        "or" => TokenKind::Or,
        "orelse" => TokenKind::Orelse,
        "return" => TokenKind::Return,
        "struct" => TokenKind::Struct,
        "true" => TokenKind::True,
        "try" => TokenKind::Try,
        "var" => TokenKind::Var,
        "while" => TokenKind::While,
        _ => return None,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}
