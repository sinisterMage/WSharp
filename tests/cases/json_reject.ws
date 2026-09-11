// What `std/json` refuses, and what it says about it.
//
// The half that matters. A reader that accepts too much passes every test
// written from the valid side, so each line below is a document some other
// reader would take and this one will not -- and the message is pinned, because
// a parser's whole job is to be good on the day it goes wrong.
//
// Every case here was run through `node`'s `JSON.parse` and `python3`'s
// `json.loads` first. All three agree except on four, each of which is a
// decision rather than an accident:
//
//   * `NaN` and `Infinity` -- python takes them, node and this do not. They are
//     not in RFC 8259 and there is nothing to write them back out as.
//   * a lone surrogate -- node and python take one, because a JavaScript or
//     Python string is UTF-16-shaped and can hold half a pair. A W# `str` is
//     UTF-8 bytes and there is no UTF-8 encoding of `\uD800` to hold, so it is
//     refused, as `serde_json` refuses it and as `std/toml` already does.
//   * nesting past `MAX_DEPTH` -- both keep going and this stops, because the
//     reader is recursive and a few hundred kilobytes of `[` would otherwise be
//     a stack overflow with no diagnostic.
//
// The position is the line *and* the byte offset, which is why every one of
// these one-line documents reports line 1 and a different number.
// expect: empty: 1 0 the document is empty
// expect: only-whitespace: 1 3 expected a value, and the document ends here
// expect: two-values: 1 2 there is more text after the end of the document
// expect: a-comment: 1 4 there is more text after the end of the document
// expect: a-byte-order-mark: 1 0 this document begins with a byte-order mark, which JSON does not allow
// expect: trailing-comma-in-an-array: 1 3 an array cannot end with a comma
// expect: trailing-comma-in-an-object: 1 7 an object cannot end with a comma
// expect: a-leading-comma: 1 1 expected a value
// expect: a-doubled-comma: 1 3 expected a value
// expect: a-missing-comma: 1 3 expected `,` or `]` in an array
// expect: a-missing-colon: 1 5 expected `:` after a member's name
// expect: an-unquoted-name: 1 1 a member's name must be a quoted string
// expect: an-unclosed-array: 1 5 expected `]` to close this array
// expect: an-unclosed-object: 1 7 expected `}` to close this object
// expect: a-mismatched-bracket: 1 2 expected `,` or `]` in an array
// expect: uppercase-true: 1 0 expected a value: the only bare words JSON has are `true`, `false` and `null`
// expect: not-a-number: 1 0 expected a value: the only bare words JSON has are `true`, `false` and `null`
// expect: infinity: 1 0 expected a value: the only bare words JSON has are `true`, `false` and `null`
// expect: undefined: 1 0 expected a value: the only bare words JSON has are `true`, `false` and `null`
// expect: a-leading-zero: 1 2 a number may not have a leading zero
// expect: a-signed-leading-zero: 1 3 a number may not have a leading zero
// expect: a-leading-plus: 1 0 a number may not begin with `+`
// expect: no-digit-before-the-point: 1 0 a number needs a digit in front of its decimal point
// expect: no-digit-after-the-point: 1 2 a number needs at least one digit after its decimal point
// expect: no-digit-after-the-exponent: 1 2 a number needs at least one digit after its exponent
// expect: a-bare-minus: 1 1 a number needs at least one digit
// expect: hexadecimal: 1 1 there is more text after the end of the document
// expect: single-quoted: 1 0 JSON has no single-quoted string
// expect: an-unclosed-string: 1 4 a string is not closed before the end of the document
// expect: a-raw-newline: 1 2 a string cannot hold a raw control character; write it as an escape
// expect: a-raw-tab: 1 2 a string cannot hold a raw control character; write it as an escape
// expect: an-unknown-escape: 1 3 unknown escape sequence: the ones JSON has are \" \\ \/ \b \f \n \r \t \uXXXX
// expect: a-short-escape: 1 5 an escape needs four hexadecimal digits after its `u`
// expect: a-lone-high-surrogate: 1 7 a high surrogate must be followed by a low surrogate
// expect: a-lone-low-surrogate: 1 7 this is a low surrogate with no high surrogate in front of it
// expect: a-high-surrogate-then-a-letter: 1 7 a high surrogate must be followed by a low surrogate
// expect: two-high-surrogates: 1 13 a high surrogate must be followed by a low surrogate
// expect: as-deep-as-it-goes: ok
// expect: one-deeper: 1 129 this document nests more than 128 deep
// expect: two-mistakes: 1 3 a number may not have a leading zero
const json = @import("std/json");
const text = @import("std/str");

/// Parse and say what came of it: `ok`, or the line, the offset and the message.
fn check(name: str, src: str) void {
    const doc = json.parse(src);
    if (doc.ok) { print(text.concat(name, ": ok")); return; }
    print(text.concat(name, text.concat(": ", text.concat(text.from_int(doc.line),
        text.concat(" ", text.concat(text.from_int(doc.at),
            text.concat(" ", doc.message)))))));
    return;
}

fn main() i64 {
    // Nothing, and too much.
    check("empty", "");
    check("only-whitespace", "   ");
    check("two-values", "1 2");
    check("a-comment", "[1] // hi");
    check("a-byte-order-mark", text.concat(bom(), "{}"));

    // Structure.
    check("trailing-comma-in-an-array", "[1,]");
    check("trailing-comma-in-an-object", "{\"a\":1,}");
    check("a-leading-comma", "[,1]");
    check("a-doubled-comma", "[1,,2]");
    check("a-missing-comma", "[1 2]");
    check("a-missing-colon", "{\"a\" 1}");
    check("an-unquoted-name", "{a:1}");
    check("an-unclosed-array", "[1, 2");
    check("an-unclosed-object", "{\"a\": 1");
    check("a-mismatched-bracket", "[1}");

    // Bare words. Only three of them exist.
    check("uppercase-true", "True");
    check("not-a-number", "NaN");
    check("infinity", "Infinity");
    check("undefined", "undefined");

    // Numbers.
    check("a-leading-zero", "01");
    check("a-signed-leading-zero", "-01");
    check("a-leading-plus", "+1");
    check("no-digit-before-the-point", ".5");
    check("no-digit-after-the-point", "5.");
    check("no-digit-after-the-exponent", "1e");
    check("a-bare-minus", "-");
    check("hexadecimal", "0x10");

    // Strings.
    check("single-quoted", "'a'");
    check("an-unclosed-string", "\"abc");
    check("a-raw-newline", "\"a\nb\"");
    check("a-raw-tab", "\"a\tb\"");
    check("an-unknown-escape", "\"\\q\"");
    check("a-short-escape", "\"\\u12\"");
    check("a-lone-high-surrogate", "\"\\ud83d\"");
    check("a-lone-low-surrogate", "\"\\ude00\"");
    check("a-high-surrogate-then-a-letter", "\"\\ud83dx\"");
    check("two-high-surrogates", "\"\\ud83d\\ud83d\"");

    // The nesting bound, from both sides of it.
    check("as-deep-as-it-goes", nested(128));
    check("one-deeper", nested(129));

    // The first failure wins: the second mistake here is never reported.
    check("two-mistakes", "[01, 02]");
    return 0;
}

/// `n` open brackets and `n` closing ones.
fn nested(n: i64) str {
    return text.concat(text.repeat("[", n), text.repeat("]", n));
}

/// A UTF-8 byte-order mark, built rather than written: a W# string literal has
/// no `\u` escape, and a mark written into this file would be invisible in it.
fn bom() str {
    return text.concat(text.from_byte(239), text.concat(text.from_byte(187), text.from_byte(191)));
}
