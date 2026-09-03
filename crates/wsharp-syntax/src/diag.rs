//! Diagnostics and their rendering.
//!
//! Hand-rolled rather than pulled from a crate: error messages are a large part
//! of how a language feels, and owning the renderer keeps the format stable and
//! test-friendly (no colour codes, deterministic output).

use crate::span::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    fn label(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
}

/// A span with an attached explanation, rendered as an underline.
#[derive(Debug, Clone)]
pub struct Label {
    pub span: Span,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    /// The span the diagnostic points at. Its message may be empty.
    pub primary: Label,
    /// Extra spans providing context, rendered after the primary one.
    pub secondary: Vec<Label>,
    pub help: Option<String>,
}

impl Diagnostic {
    pub fn error(span: Span, message: impl Into<String>) -> Diagnostic {
        Diagnostic {
            severity: Severity::Error,
            message: message.into(),
            primary: Label {
                span,
                message: String::new(),
            },
            secondary: Vec::new(),
            help: None,
        }
    }

    /// Set the text rendered under the primary span's carets.
    pub fn label(mut self, message: impl Into<String>) -> Diagnostic {
        self.primary.message = message.into();
        self
    }

    pub fn secondary(mut self, span: Span, message: impl Into<String>) -> Diagnostic {
        self.secondary.push(Label {
            span,
            message: message.into(),
        });
        self
    }

    pub fn help(mut self, help: impl Into<String>) -> Diagnostic {
        self.help = Some(help.into());
        self
    }
}

/// Source text plus a precomputed line index, so spans can be turned into
/// line/column pairs without rescanning the file.
pub struct SourceFile {
    pub name: String,
    pub text: String,
    /// Byte offset of the first character of each line.
    line_starts: Vec<u32>,
}

impl SourceFile {
    pub fn new(name: impl Into<String>, text: impl Into<String>) -> SourceFile {
        let text = text.into();
        let mut line_starts = vec![0u32];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i as u32 + 1);
            }
        }
        SourceFile {
            name: name.into(),
            text,
            line_starts,
        }
    }

    /// 1-based line and column for a byte offset. Column counts characters, not
    /// bytes, so multi-byte source still points at the right place.
    pub fn line_col(&self, offset: u32) -> (usize, usize) {
        let line = match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let start = self.line_starts[line] as usize;
        let end = (offset as usize).min(self.text.len());
        let col = self.text[start..end].chars().count() + 1;
        (line + 1, col)
    }

    /// The text of a 1-based line, without its trailing newline.
    pub fn line_text(&self, line: usize) -> &str {
        let start = self.line_starts[line - 1] as usize;
        let end = self
            .line_starts
            .get(line)
            .map(|&e| e as usize)
            .unwrap_or(self.text.len());
        self.text[start..end].trim_end_matches(['\n', '\r'])
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }
}

/// Render a diagnostic in the style of
///
/// ```text
/// error: type mismatch
///   --> examples/fib.ws:4:12
///    |
///  4 |     return n + true;
///    |            ^^^^^^^^ expected i64, found bool
///    |
///    = help: ...
/// ```
pub fn render(file: &SourceFile, diag: &Diagnostic) -> String {
    let mut out = String::new();
    out.push_str(diag.severity.label());
    out.push_str(": ");
    out.push_str(&diag.message);
    out.push('\n');

    let (line, col) = file.line_col(diag.primary.span.start);
    // Gutter is sized off the largest line number we will print.
    let widest = std::iter::once(diag.primary.span)
        .chain(diag.secondary.iter().map(|l| l.span))
        .map(|s| file.line_col(s.start).0)
        .max()
        .unwrap_or(line);
    let gutter = widest.to_string().len();

    out.push_str(&format!(
        "{:pad$}--> {}:{}:{}\n",
        "",
        file.name,
        line,
        col,
        pad = gutter + 1
    ));
    push_bar(&mut out, gutter);
    push_snippet(&mut out, file, &diag.primary, gutter);
    for label in &diag.secondary {
        push_bar(&mut out, gutter);
        push_snippet(&mut out, file, label, gutter);
    }

    if let Some(help) = &diag.help {
        push_bar(&mut out, gutter);
        out.push_str(&format!("{:pad$}= help: {}\n", "", help, pad = gutter + 1));
    }
    out
}

fn push_bar(out: &mut String, gutter: usize) {
    out.push_str(&format!("{:pad$}|\n", "", pad = gutter + 1));
}

fn push_snippet(out: &mut String, file: &SourceFile, label: &Label, gutter: usize) {
    let (line, col) = file.line_col(label.span.start);
    let text = file.line_text(line);
    out.push_str(&format!("{:>gutter$} | {}\n", line, text, gutter = gutter));

    // A span may run past the end of its first line (a multi-line construct);
    // clamp the underline so it never spills into the next line's text.
    let line_end_col = text.chars().count() + 1;
    let (end_line, end_col) = file.line_col(label.span.end);
    let end = if end_line == line {
        end_col
    } else {
        line_end_col
    };
    let width = end.saturating_sub(col).max(1);

    let carets = "^".repeat(width);
    out.push_str(&format!("{:pad$}| ", "", pad = gutter + 1));
    for _ in 1..col {
        out.push(' ');
    }
    out.push_str(&carets);
    if !label.message.is_empty() {
        out.push(' ');
        out.push_str(&label.message);
    }
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_col_is_one_based() {
        let f = SourceFile::new("t.ws", "abc\ndef\n");
        assert_eq!(f.line_col(0), (1, 1));
        assert_eq!(f.line_col(2), (1, 3));
        assert_eq!(f.line_col(4), (2, 1));
        assert_eq!(f.line_col(6), (2, 3));
    }

    #[test]
    fn line_col_counts_characters_not_bytes() {
        let f = SourceFile::new("t.ws", "\"שלום\" x");
        // The `x` is many bytes in but only 8 characters across.
        let offset = f.text.find('x').unwrap() as u32;
        assert_eq!(f.line_col(offset), (1, 8));
    }

    #[test]
    fn renders_caret_under_span() {
        let f = SourceFile::new("t.ws", "const x = 1;\nconst y = 2;\n");
        let d = Diagnostic::error(Span::new(19, 20), "bad").label("here");
        let s = render(&f, &d);
        assert!(s.starts_with("error: bad\n"), "{s}");
        assert!(s.contains("--> t.ws:2:7"), "{s}");
        assert!(s.contains("2 | const y = 2;"), "{s}");
        assert!(s.contains("^ here"), "{s}");
    }

    #[test]
    fn multiline_span_underline_stops_at_end_of_line() {
        let f = SourceFile::new("t.ws", "fn a() {\n  return 1;\n}\n");
        let d = Diagnostic::error(Span::new(0, 22), "whole fn");
        let s = render(&f, &d);
        // 8 characters on line 1 from column 1, not 22.
        assert!(s.contains("^^^^^^^^\n"), "{s}");
    }
}
