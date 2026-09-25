//! Bounded malformed-input coverage. Keep the seed and case number in failures
//! so a discovered input can become a small, named regression.
use wsharp_syntax::{SourceMap, Span, lexer, parse_at, render};

const SEED: u64 = 0x5753_4841_5250;
const CASES: usize = 512;

fn next(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

fn check_span(source: &str, span: Span, base: u32) {
    assert!(span.start >= base && span.end >= span.start, "{span:?}");
    let start = (span.start - base) as usize;
    let end = (span.end - base) as usize;
    assert!(end <= source.len(), "{span:?} in {source:?}");
    assert!(source.is_char_boundary(start), "{span:?} in {source:?}");
    assert!(source.is_char_boundary(end), "{span:?} in {source:?}");
}

#[test]
fn generated_utf8_inputs_keep_tokens_and_diagnostics_on_source_boundaries() {
    // Mix incomplete valid forms, punctuation, bad escapes and multi-byte
    // characters. These are robustness assertions, not new syntax rules.
    let pieces = [
        "fn", "main", "()", "{", "}", "[", "]", "(", ")", ";", ":", ",", "const", "var", "return",
        "if", "else", "0", "0xff", "0b2", "1.5", "_", "=", "+", "-", "!", "|", "?", "@", "/", "//",
        "\\", "\"", "\n", "\r", "\t", " ", "é", "中", "🦀", "\0",
    ];
    let mut state = SEED;
    for case in 0..CASES {
        let count = (next(&mut state) % 65) as usize;
        let mut source = String::new();
        for _ in 0..count {
            source.push_str(pieces[(next(&mut state) % pieces.len() as u64) as usize]);
        }
        let result = std::panic::catch_unwind(|| {
            let mut map = SourceMap::new();
            map.add("earlier.ws", "// another module\n");
            let base = map.add("generated.ws", source.clone());
            let (tokens, _) = lexer::lex_at(&source, base);
            let mut end = base;
            for token in tokens {
                check_span(&source, token.span, base);
                assert!(token.span.start >= end);
                end = token.span.end;
            }
            assert_eq!(end, base + source.len() as u32);
            let (_, diagnostics) = parse_at(&source, base);
            for diagnostic in diagnostics {
                check_span(&source, diagnostic.primary.span, base);
                for label in &diagnostic.secondary {
                    check_span(&source, label.span, base);
                }
                assert!(!render(&map, &diagnostic).is_empty());
            }
        });
        assert!(
            result.is_ok(),
            "seed {SEED:#x}, case {case}, source {source:?}"
        );
    }
}
