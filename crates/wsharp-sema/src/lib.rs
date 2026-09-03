//! Name resolution and Hindley-Milner type inference for W#.

pub mod hir;
pub mod infer;
pub mod layout;
pub mod mono;
pub mod ty;

pub use infer::{Analysis, analyze};
pub use mono::{MonoResult, monomorphize};
pub use ty::{Scheme, TyCon, Type, TypeStore};
