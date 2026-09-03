//! Byte offsets into a single source file.

use std::ops::Range;

/// A half-open byte range `[start, end)` into the source text.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    /// A span pointing at nothing, for declarations the compiler synthesises
    /// rather than reads -- the builtin status types, for instance. A
    /// diagnostic should never carry one: there is no source to underline.
    pub const EMPTY: Span = Span { start: 0, end: 0 };

    pub fn new(start: usize, end: usize) -> Span {
        debug_assert!(start <= end);
        Span {
            start: start as u32,
            end: end as u32,
        }
    }

    /// A span covering both `self` and `other` (and everything between).
    pub fn to(self, other: Span) -> Span {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    pub fn range(self) -> Range<usize> {
        self.start as usize..self.end as usize
    }

    pub fn len(self) -> usize {
        (self.end - self.start) as usize
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }
}

impl std::fmt::Debug for Span {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}..{}", self.start, self.end)
    }
}

/// A name plus the span it was written at.
#[derive(Clone, PartialEq, Eq)]
pub struct Ident {
    pub name: Box<str>,
    pub span: Span,
}

impl Ident {
    pub fn new(name: impl Into<Box<str>>, span: Span) -> Ident {
        Ident {
            name: name.into(),
            span,
        }
    }

    pub fn as_str(&self) -> &str {
        &self.name
    }
}

impl std::fmt::Debug for Ident {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

impl std::fmt::Display for Ident {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)
    }
}
