//! Source text to tokens.
//!
//! The lexer never bails out: on a bad character or malformed literal it
//! records a diagnostic, makes a plausible recovery, and keeps going, so a
//! single typo does not hide every later error.

use crate::diag::Diagnostic;
use crate::span::Span;
use crate::token::{Token, TokenKind, keyword};

pub struct Lexer<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
    diags: Vec<Diagnostic>,
}

/// Tokenize `src`. Always returns a token stream terminated by `Eof`; any
/// problems are reported in the accompanying diagnostics.
pub fn lex(src: &str) -> (Vec<Token>, Vec<Diagnostic>) {
    lex_at(src, 0)
}

/// Lex, offsetting every span by `base` so that it names a position in the
/// whole program rather than in this file alone.
///
/// Shifting afterwards rather than threading `base` through the scanner keeps
/// every `Span::new` in here about the text in front of it, and leaves exactly
/// one place where a file's position in the program is applied.
/// See [`crate::diag::SourceMap`].
pub fn lex_at(src: &str, base: u32) -> (Vec<Token>, Vec<Diagnostic>) {
    let (mut tokens, mut diags) = scan(src);
    if base != 0 {
        for token in &mut tokens {
            token.span = shift(token.span, base);
        }
        for diag in &mut diags {
            shift_diagnostic(diag, base);
        }
    }
    (tokens, diags)
}

pub(crate) fn shift(span: Span, base: u32) -> Span {
    Span {
        start: span.start + base,
        end: span.end + base,
    }
}

pub(crate) fn shift_diagnostic(diag: &mut Diagnostic, base: u32) {
    diag.primary.span = shift(diag.primary.span, base);
    for label in &mut diag.secondary {
        label.span = shift(label.span, base);
    }
}

fn scan(src: &str) -> (Vec<Token>, Vec<Diagnostic>) {
    let mut lexer = Lexer {
        src,
        bytes: src.as_bytes(),
        pos: 0,
        diags: Vec::new(),
    };
    let mut tokens = Vec::new();
    loop {
        let token = lexer.next_token();
        let done = token.kind == TokenKind::Eof;
        tokens.push(token);
        if done {
            break;
        }
    }
    (tokens, lexer.diags)
}

impl<'a> Lexer<'a> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.pos + offset).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let b = self.peek()?;
        // Advance by a whole UTF-8 sequence so `pos` stays on a char boundary.
        self.pos += if b < 0x80 {
            1
        } else {
            self.src[self.pos..]
                .chars()
                .next()
                .map_or(1, char::len_utf8)
        };
        Some(b)
    }

    fn eat(&mut self, b: u8) -> bool {
        if self.peek() == Some(b) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(b' ' | b'\t' | b'\r' | b'\n') => {
                    self.pos += 1;
                }
                Some(b'/') if self.peek_at(1) == Some(b'/') => {
                    while let Some(b) = self.peek() {
                        if b == b'\n' {
                            break;
                        }
                        self.pos += 1;
                    }
                }
                _ => return,
            }
        }
    }

    fn next_token(&mut self) -> Token {
        // A loop rather than a recursive retry after a bad character: a long
        // run of them is user input, and each one would otherwise cost a stack
        // frame.
        loop {
            self.skip_trivia();
            let start = self.pos;
            let Some(b) = self.peek() else {
                return Token {
                    kind: TokenKind::Eof,
                    span: Span::new(start, start),
                };
            };

            let kind = match b {
                b'0'..=b'9' => return self.number(start),
                b'"' => return self.string(start),
                b'_' | b'a'..=b'z' | b'A'..=b'Z' => return self.ident(start),
                _ => {
                    self.bump();
                    match b {
                        b'(' => TokenKind::LParen,
                        b')' => TokenKind::RParen,
                        b'{' => TokenKind::LBrace,
                        b'}' => TokenKind::RBrace,
                        b'[' => TokenKind::LBracket,
                        b']' => TokenKind::RBracket,
                        b',' => TokenKind::Comma,
                        b';' => TokenKind::Semi,
                        b':' => TokenKind::Colon,
                        b'|' => self.maybe_eq(TokenKind::PipeEq, TokenKind::Pipe),
                        b'&' => self.maybe_eq(TokenKind::AmpEq, TokenKind::Amp),
                        b'^' => self.maybe_eq(TokenKind::CaretEq, TokenKind::Caret),
                        b'~' => TokenKind::Tilde,
                        b'?' => TokenKind::Question,
                        b'@' => TokenKind::At,
                        b'.' => {
                            if self.eat(b'?') {
                                TokenKind::DotQuestion
                            } else {
                                TokenKind::Dot
                            }
                        }
                        b'+' => self.maybe_eq(TokenKind::PlusEq, TokenKind::Plus),
                        b'-' => self.maybe_eq(TokenKind::MinusEq, TokenKind::Minus),
                        b'*' => self.maybe_eq(TokenKind::StarEq, TokenKind::Star),
                        b'/' => self.maybe_eq(TokenKind::SlashEq, TokenKind::Slash),
                        b'%' => self.maybe_eq(TokenKind::PercentEq, TokenKind::Percent),
                        b'=' => self.maybe_eq(TokenKind::EqEq, TokenKind::Eq),
                        b'!' => self.maybe_eq(TokenKind::BangEq, TokenKind::Bang),
                        // Three characters deep, so the longer token has to
                        // be tried first at each step: `<<=` before `<<`,
                        // and `<<` before `<=`. `maybe_eq` only knows how to
                        // look one character ahead.
                        b'<' => self.shift_or(
                            TokenKind::ShlEq,
                            TokenKind::Shl,
                            TokenKind::LtEq,
                            TokenKind::Lt,
                            b'<',
                        ),
                        b'>' => self.shift_or(
                            TokenKind::ShrEq,
                            TokenKind::Shr,
                            TokenKind::GtEq,
                            TokenKind::Gt,
                            b'>',
                        ),
                        _ => {
                            let span = Span::new(start, self.pos);
                            let ch = self.src[start..self.pos].chars().next().unwrap_or('?');
                            self.diags.push(
                                Diagnostic::error(span, format!("unexpected character `{ch}`"))
                                    .label("not valid in W# source"),
                            );
                            // Skip it and carry on rather than derailing the stream.
                            continue;
                        }
                    }
                }
            };
            return Token {
                kind,
                span: Span::new(start, self.pos),
            };
        }
    }

    fn maybe_eq(&mut self, with_eq: TokenKind, without: TokenKind) -> TokenKind {
        if self.eat(b'=') { with_eq } else { without }
    }

    /// `<`, `<<`, `<<=` and `<=` from one leading `<` -- and the same for `>`.
    ///
    /// The doubled form is checked first because it is longer; `<=` is only
    /// reachable once the second `<` is ruled out, which is what keeps
    /// comparison working unchanged.
    fn shift_or(
        &mut self,
        shift_eq: TokenKind,
        shift: TokenKind,
        cmp_eq: TokenKind,
        cmp: TokenKind,
        doubled: u8,
    ) -> TokenKind {
        if self.eat(doubled) {
            self.maybe_eq(shift_eq, shift)
        } else {
            self.maybe_eq(cmp_eq, cmp)
        }
    }

    fn ident(&mut self, start: usize) -> Token {
        while matches!(
            self.peek(),
            Some(b'_' | b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9')
        ) {
            self.pos += 1;
        }
        let span = Span::new(start, self.pos);
        let word = &self.src[start..self.pos];
        let kind = keyword(word).unwrap_or_else(|| TokenKind::Ident(word.into()));
        Token { kind, span }
    }

    /// Integers (decimal, `0x`, `0b`, `0o`) and floats, with `_` separators as
    /// in Zig.
    fn number(&mut self, start: usize) -> Token {
        let (radix, digits_start) = match (self.peek(), self.peek_at(1)) {
            (Some(b'0'), Some(b'x' | b'X')) => (16, start + 2),
            (Some(b'0'), Some(b'b' | b'B')) => (2, start + 2),
            (Some(b'0'), Some(b'o' | b'O')) => (8, start + 2),
            _ => (10, start),
        };
        self.pos = digits_start;

        let mut is_float = false;
        // The first digit that does not belong to this radix, if any. It is
        // consumed with the rest so the literal stays one token, and reported
        // by itself: `0b12` is a stray `2`, not an out-of-range number.
        let mut bad_digit: Option<usize> = None;
        while let Some(b) = self.peek() {
            match b {
                b'_' => self.pos += 1,
                b'.' if radix == 10 && !is_float => {
                    // Only a fractional point if a digit follows; otherwise this
                    // is field access on an integer and belongs to the parser.
                    if matches!(self.peek_at(1), Some(b'0'..=b'9')) {
                        is_float = true;
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
                _ if (b as char).is_digit(radix) => self.pos += 1,
                b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F' if radix != 10 => {
                    bad_digit.get_or_insert(self.pos);
                    self.pos += 1;
                }
                _ => break,
            }
        }

        let span = Span::new(start, self.pos);
        let text = &self.src[digits_start..self.pos];
        let cleaned: String = text.chars().filter(|&c| c != '_').collect();

        if let Some(at) = bad_digit {
            let digit = self.bytes[at] as char;
            let (name, digits) = match radix {
                2 => ("binary", "`0` and `1`"),
                8 => ("octal", "`0` to `7`"),
                _ => ("hexadecimal", "`0` to `9` and `a` to `f`"),
            };
            self.diags.push(
                Diagnostic::error(
                    Span::new(at, at + 1),
                    format!("digit `{digit}` is not valid in a {name} literal"),
                )
                .help(format!("{name} digits are {digits}")),
            );
            return Token {
                kind: TokenKind::Int(0),
                span,
            };
        }

        if cleaned.is_empty() {
            self.diags.push(
                Diagnostic::error(span, "numeric literal has no digits")
                    .label("expected digits here"),
            );
            return Token {
                kind: TokenKind::Int(0),
                span,
            };
        }

        let kind = if is_float {
            match cleaned.parse::<f64>() {
                Ok(v) => TokenKind::Float(v),
                Err(_) => {
                    self.diags.push(
                        Diagnostic::error(span, "invalid float literal")
                            .label("cannot be parsed as f64"),
                    );
                    TokenKind::Float(0.0)
                }
            }
        } else {
            // Parsed as `i128` and bounded by `u64::MAX` rather than by
            // `i64::MAX`, because a literal has no type yet: `0xFFFF_FFFF_FFFF_FFFF`
            // is a perfectly good `u64` and only inference can know that.
            // Whether the value fits the type it is *used* at is checked
            // there, where the type is known.
            //
            // A literal is always non-negative here -- `-1` is unary minus
            // applied to `1` -- so the upper bound is the only one.
            match i128::from_str_radix(&cleaned, radix) {
                Ok(v) if v <= u64::MAX as i128 => TokenKind::Int(v),
                _ => {
                    self.diags.push(
                        Diagnostic::error(span, "integer literal out of range")
                            .label("does not fit in 64 bits")
                            .help("the widest integer types W# has are `i64` and `u64`"),
                    );
                    TokenKind::Int(0)
                }
            }
        };
        Token { kind, span }
    }

    fn string(&mut self, start: usize) -> Token {
        self.pos += 1; // opening quote
        let mut value = String::new();
        loop {
            let Some(b) = self.peek() else {
                let span = Span::new(start, self.pos);
                self.diags.push(
                    Diagnostic::error(span, "unterminated string literal")
                        .label("string starts here"),
                );
                break;
            };
            match b {
                b'"' => {
                    self.pos += 1;
                    break;
                }
                b'\n' => {
                    let span = Span::new(start, self.pos);
                    self.diags.push(
                        Diagnostic::error(span, "unterminated string literal")
                            .label("string starts here")
                            .help("strings cannot span multiple lines"),
                    );
                    break;
                }
                b'\\' => {
                    let esc_start = self.pos;
                    self.pos += 1;
                    match self.peek() {
                        Some(b'n') => {
                            value.push('\n');
                            self.pos += 1;
                        }
                        Some(b't') => {
                            value.push('\t');
                            self.pos += 1;
                        }
                        Some(b'r') => {
                            value.push('\r');
                            self.pos += 1;
                        }
                        Some(b'0') => {
                            value.push('\0');
                            self.pos += 1;
                        }
                        Some(b'\\') => {
                            value.push('\\');
                            self.pos += 1;
                        }
                        Some(b'"') => {
                            value.push('"');
                            self.pos += 1;
                        }
                        _ => {
                            // `bump`, not `pos += 1`: the character after the
                            // backslash may be several bytes long, and stopping
                            // inside it would make the next slice panic.
                            self.bump();
                            let span = Span::new(esc_start, self.pos);
                            self.diags.push(
                                Diagnostic::error(span, "unknown escape sequence")
                                    .label("not a recognised escape")
                                    .help("valid escapes are \\n \\t \\r \\0 \\\\ \\\""),
                            );
                        }
                    }
                }
                _ => {
                    let ch_start = self.pos;
                    self.bump();
                    value.push_str(&self.src[ch_start..self.pos]);
                }
            }
        }
        Token {
            kind: TokenKind::Str(value.into()),
            span: Span::new(start, self.pos),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<TokenKind> {
        let (tokens, diags) = lex(src);
        assert!(
            diags.is_empty(),
            "unexpected diagnostics: {:?}",
            diags[0].message
        );
        tokens.into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn lexes_a_small_function() {
        assert_eq!(
            kinds("fn f(a) i64 { return a + 1; }"),
            vec![
                TokenKind::Fn,
                TokenKind::Ident("f".into()),
                TokenKind::LParen,
                TokenKind::Ident("a".into()),
                TokenKind::RParen,
                TokenKind::Ident("i64".into()),
                TokenKind::LBrace,
                TokenKind::Return,
                TokenKind::Ident("a".into()),
                TokenKind::Plus,
                TokenKind::Int(1),
                TokenKind::Semi,
                TokenKind::RBrace,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn distinguishes_two_character_operators() {
        assert_eq!(
            kinds("== != <= >= += -= *= /= %= .? ."),
            vec![
                TokenKind::EqEq,
                TokenKind::BangEq,
                TokenKind::LtEq,
                TokenKind::GtEq,
                TokenKind::PlusEq,
                TokenKind::MinusEq,
                TokenKind::StarEq,
                TokenKind::SlashEq,
                TokenKind::PercentEq,
                TokenKind::DotQuestion,
                TokenKind::Dot,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn number_bases_and_separators() {
        assert_eq!(
            kinds("0xff 0b1010 0o17 1_000 3.5"),
            vec![
                TokenKind::Int(255),
                TokenKind::Int(10),
                TokenKind::Int(15),
                TokenKind::Int(1000),
                TokenKind::Float(3.5),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn dot_after_integer_is_field_access_not_a_float() {
        // `1.foo` must lex as Int, Dot, Ident -- not as a malformed float.
        assert_eq!(
            kinds("1.foo"),
            vec![
                TokenKind::Int(1),
                TokenKind::Dot,
                TokenKind::Ident("foo".into()),
                TokenKind::Eof
            ]
        );
    }

    #[test]
    fn comments_and_strings() {
        assert_eq!(
            kinds("// skipped\n\"a\\nb\" // also skipped"),
            vec![TokenKind::Str("a\nb".into()), TokenKind::Eof]
        );
    }

    #[test]
    fn keywords_are_not_identifiers() {
        assert_eq!(
            kinds("try catch orelse error null"),
            vec![
                TokenKind::Try,
                TokenKind::Catch,
                TokenKind::Orelse,
                TokenKind::Error,
                TokenKind::Null,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn primitive_type_names_lex_as_identifiers() {
        assert_eq!(
            kinds("i64 f64 bool void str"),
            vec![
                TokenKind::Ident("i64".into()),
                TokenKind::Ident("f64".into()),
                TokenKind::Ident("bool".into()),
                TokenKind::Ident("void".into()),
                TokenKind::Ident("str".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn recovers_from_bad_character() {
        let (tokens, diags) = lex("a # b");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("unexpected character"));
        // Lexing continued past the bad byte.
        assert_eq!(
            tokens.into_iter().map(|t| t.kind).collect::<Vec<_>>(),
            vec![
                TokenKind::Ident("a".into()),
                TokenKind::Ident("b".into()),
                TokenKind::Eof
            ]
        );
    }

    #[test]
    fn reports_unterminated_string() {
        let (_, diags) = lex("\"abc");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("unterminated"));
    }

    #[test]
    fn reports_integer_overflow() {
        let (_, diags) = lex("99999999999999999999");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("out of range"));
    }

    /// The bound is `u64::MAX`, not `i64::MAX`: a literal has no type yet, and
    /// `0xFFFF_FFFF_FFFF_FFFF` is a perfectly good `u64`. Whether the value
    /// fits the type it is *used* at is inference's question.
    #[test]
    fn a_literal_may_fill_a_u64() {
        let (tokens, diags) = lex("0xFFFF_FFFF_FFFF_FFFF");
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(tokens[0].kind, TokenKind::Int(u64::MAX as i128));

        let (_, diags) = lex("18446744073709551616");
        assert_eq!(diags.len(), 1, "one past `u64::MAX` is still out of range");
    }

    #[test]
    fn bitwise_operators_lex_without_disturbing_comparison() {
        let (tokens, diags) = lex("a & b | c ^ ~d << 1 >> 2 <= 3 >= 4 < 5 > 6");
        assert!(diags.is_empty(), "{diags:?}");
        let kinds: Vec<&TokenKind> = tokens.iter().map(|t| &t.kind).collect();
        let text: Vec<String> = kinds
            .iter()
            .filter(|k| !matches!(k, TokenKind::Ident(_) | TokenKind::Int(_) | TokenKind::Eof))
            .map(|k| k.text().to_string())
            .collect();
        assert_eq!(text, ["&", "|", "^", "~", "<<", ">>", "<=", ">=", "<", ">"]);

        let (tokens, diags) = lex("a &= b |= c ^= d <<= e >>= f");
        assert!(diags.is_empty(), "{diags:?}");
        let text: Vec<String> = tokens
            .iter()
            .map(|t| &t.kind)
            .filter(|k| !matches!(k, TokenKind::Ident(_) | TokenKind::Eof))
            .map(|k| k.text().to_string())
            .collect();
        assert_eq!(text, ["&=", "|=", "^=", "<<=", ">>="]);
    }

    #[test]
    fn a_digit_from_the_wrong_radix_is_named_not_reported_as_overflow() {
        let src = "0b12";
        let (tokens, diags) = lex(src);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(
            diags[0].message,
            "digit `2` is not valid in a binary literal"
        );
        // The caret sits on the offending digit, not the whole literal.
        assert_eq!(&src[diags[0].primary.span.range()], "2");
        // The literal is still one token, so the parser sees no split.
        assert_eq!(tokens[0].kind, TokenKind::Int(0));
        assert_eq!(&src[tokens[0].span.range()], "0b12");

        let (_, diags) = lex("0o9");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("octal"), "{}", diags[0].message);
    }

    #[test]
    fn bad_escape_before_a_multibyte_character_does_not_panic() {
        // The byte after `\` starts a two-byte character; skipping only one
        // byte used to leave `pos` inside it and panic on the next slice.
        let src = "\"\\é\"";
        let (tokens, diags) = lex(src);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("unknown escape"));
        assert_eq!(&src[diags[0].primary.span.range()], "\\é");
        assert!(matches!(tokens[0].kind, TokenKind::Str(_)));
        assert_eq!(tokens[1].kind, TokenKind::Eof);
    }

    #[test]
    fn a_long_run_of_bad_characters_does_not_overflow_the_stack() {
        // Each bad byte used to recurse once, so this would overflow.
        let src = "#".repeat(100_000);
        let (tokens, diags) = lex(&src);
        assert_eq!(diags.len(), 100_000);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind, TokenKind::Eof);
    }

    #[test]
    fn spans_point_at_the_right_text() {
        let src = "const x = 42;";
        let (tokens, _) = lex(src);
        let int = tokens
            .iter()
            .find(|t| matches!(t.kind, TokenKind::Int(_)))
            .unwrap();
        assert_eq!(&src[int.span.range()], "42");
    }

    #[test]
    fn multibyte_source_keeps_char_boundaries() {
        // Non-ASCII inside a string must not desynchronise `pos`.
        let (tokens, diags) = lex("\"שלום\" + 1");
        assert!(diags.is_empty());
        assert_eq!(tokens[0].kind, TokenKind::Str("שלום".into()));
        assert_eq!(tokens[1].kind, TokenKind::Plus);
        assert_eq!(tokens[2].kind, TokenKind::Int(1));
    }
}
