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
    /// Where this file's text begins in the whole program's offset space. A
    /// [`Span`] is global, so `base` is what turns one back into a position in
    /// this file. See [`SourceMap`].
    pub base: u32,
    /// Byte offset of the first character of each line, relative to `base`.
    line_starts: Vec<u32>,
}

impl SourceFile {
    pub fn new(name: impl Into<String>, text: impl Into<String>) -> SourceFile {
        SourceFile::at(name, text, 0)
    }

    pub fn at(name: impl Into<String>, text: impl Into<String>, base: u32) -> SourceFile {
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
            base,
            line_starts,
        }
    }

    /// The half-open range of global offsets this file covers, the byte just
    /// past its end included so that a span pointing at end-of-file lands here.
    fn covers(&self, offset: u32) -> bool {
        offset >= self.base && offset <= self.base + self.text.len() as u32
    }

    /// 1-based line and column for a byte offset. Column counts characters, not
    /// bytes, so multi-byte source still points at the right place.
    pub fn line_col(&self, offset: u32) -> (usize, usize) {
        let offset = offset.saturating_sub(self.base);
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

/// Every file that went into one program, laid end to end in a single offset
/// space.
///
/// A [`Span`] stays two `u32`s with no file in it. Widening it would touch
/// every node in the syntax tree and every diagnostic, to carry a number that
/// only the renderer ever reads -- so each file is given a base offset instead
/// and a span's file is found by which range it falls in. The first file starts
/// at 1, which keeps offset 0 meaning [`Span::EMPTY`]: no source at all.
#[derive(Default)]
pub struct SourceMap {
    files: Vec<SourceFile>,
}

impl SourceMap {
    pub fn new() -> SourceMap {
        SourceMap::default()
    }

    /// Add a file and return the base its spans must be offset by.
    pub fn add(&mut self, name: impl Into<String>, text: impl Into<String>) -> u32 {
        let base = match self.files.last() {
            Some(last) => last.base + last.text.len() as u32 + 1,
            None => 1,
        };
        self.files.push(SourceFile::at(name, text, base));
        base
    }

    pub fn files(&self) -> &[SourceFile] {
        &self.files
    }

    /// The file a span points into, or `None` for [`Span::EMPTY`] and anything
    /// else that names no source.
    pub fn file(&self, offset: u32) -> Option<&SourceFile> {
        self.files.iter().find(|f| f.covers(offset))
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
pub fn render(map: &SourceMap, diag: &Diagnostic) -> String {
    match map.file(diag.primary.span.start) {
        Some(file) => render_in(file, diag),
        // A diagnostic should always carry a real span; if one does not, say
        // what is wrong rather than pointing at an arbitrary file.
        None => format!("{}: {}\n", diag.severity.label(), diag.message),
    }
}

fn render_in(file: &SourceFile, diag: &Diagnostic) -> String {
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
        let s = render_in(&f, &d);
        assert!(s.starts_with("error: bad\n"), "{s}");
        assert!(s.contains("--> t.ws:2:7"), "{s}");
        assert!(s.contains("2 | const y = 2;"), "{s}");
        assert!(s.contains("^ here"), "{s}");
    }

    #[test]
    fn secondary_labels_render_after_the_primary_with_their_own_snippet() {
        let f = SourceFile::new("t.ws", "const a = 1;\nconst a = 2;\n");
        let d = Diagnostic::error(Span::new(19, 20), "`a` is declared more than once")
            .secondary(Span::new(6, 7), "first declared here")
            .help("pick another name");
        let s = render_in(&f, &d);
        let expected = "\
error: `a` is declared more than once
  --> t.ws:2:7
  |
2 | const a = 2;
  |       ^
  |
1 | const a = 1;
  |       ^ first declared here
  |
  = help: pick another name
";
        assert_eq!(s, expected);
    }

    #[test]
    fn multiline_span_underline_stops_at_end_of_line() {
        let f = SourceFile::new("t.ws", "fn a() {\n  return 1;\n}\n");
        let d = Diagnostic::error(Span::new(0, 22), "whole fn");
        let s = render_in(&f, &d);
        // 8 characters on line 1 from column 1, not 22.
        assert!(s.contains("^^^^^^^^\n"), "{s}");
    }
}
