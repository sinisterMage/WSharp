//! Lexing, parsing and diagnostics for W#.
//!
//! This crate knows nothing about types or code generation: it turns source
//! text into an AST and reports syntax errors.

pub mod ast;
pub mod diag;
pub mod dump;
pub mod lexer;
pub mod parser;
pub mod span;
pub mod token;

pub use diag::{Diagnostic, Severity, SourceFile, SourceMap, render};
pub use parser::{parse, parse_at};
pub use span::{Ident, Span};
