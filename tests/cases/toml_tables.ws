// TOML's structure, and the redefinition rules that are the part nobody
// expects to be hard. A table may be created implicitly by a header below it
// and defined later; one already defined by its own header may not be defined
// again; one written `{ .. }` may not be added to at all; and one a dotted key
// brought into being may not afterwards be named by a header. Three flags on
// `Table` are exactly those rules.
//
// `parse` does not raise. An error union carries a tag and nothing else, and
// "BadFormat" is not a thing to hand somebody holding a 200-line manifest, so
// the answer is a table or a message and the line it happened on.
// expect: nested: 2
// expect: super-table-later: ok
// expect: array-of-tables: 6
// expect: into-the-last: 6
// expect: dotted-key: 1
// expect: under-a-dotted-key: 4
// expect: comments-and-blanks: ok
// expect: crlf: ok
// expect: empty: ok
// expect: table-twice: 2 `a` is defined more than once
// expect: key-twice: 2 `a` is defined more than once
// expect: header-over-a-dotted-key: 3 `apple` is defined more than once
// expect: extending-an-inline-table: 2 `a` was written as an inline table, which is closed
// expect: table-over-an-array: 2 `a` is not a table
// expect: appending-to-a-written-array: 2 `a` is a static array and cannot be extended
// expect: a-key-is-not-a-table: 3 `b` is not a table
// expect: two-statements-on-one-line: 1 expected a newline after this
// expect: a-lone-carriage-return: 1 expected a newline after this
// expect: an-unclosed-array: 2 expected `,` or `]` in an array
// expect: an-unclosed-header: 1 expected `]` to close a table header
// expect: no-value: 1 expected a value
// expect: no-key: 1 expected a key
// expect: a-trailing-comma-in-an-inline-table: 1 an inline table cannot end with a comma
// expect: an-unknown-escape: 1 unknown escape sequence: the ones TOML has are \b \t \n \f \r \" \\ \uXXXX \UXXXXXXXX
// expect: a-surrogate: 1 an escaped code point must be a scalar value
// expect: a-leading-zero: 1 an integer may not have a leading zero
// expect: a-doubled-underscore: 1 an underscore in a number must sit between two digits
// expect: a-signed-hexadecimal: 1 a hexadecimal, octal or binary number takes no sign
// expect: the-thirtieth-of-february: 1 this is not a valid date
// expect: an-hour-of-twenty-five: 1 this is not a valid time
// expect: a-string-across-two-lines: 1 a basic string is not closed before the end of its line
const text = @import("std/str");
const toml = @import("std/toml");

/// Parse and say what came of it: `ok`, or the line and the message.
fn check(name: str, src: str) void {
    const doc = toml.parse(src);
    if (doc.ok) { print(text.concat(name, ": ok")); return; }
    print(text.concat(name, text.concat(": ", text.concat(text.from_int(doc.line),
        text.concat(" ", doc.message)))));
    return;
}

/// Parse, look a dotted path up, and say whether it is there.
fn found(name: str, src: str, path: str) void {
    const doc = toml.parse(src);
    if (!doc.ok) { print(text.concat(name, text.concat(": failed -- ", doc.message))); return; }
    if (toml.lookup(doc.root, path)) |v| {
        print(text.concat(name, text.concat(": ", text.from_int(toml.kind(v)))));
    } else {
        print(text.concat(name, ": missing"));
    }
    return;
}

fn main() i64 {
    // A header names a table; a header below one creates it on the way.
    found("nested", "[a.b.c]\nx = 1\n", "a.b.c.x");
    check("super-table-later", "[a.b]\nx = 1\n[a]\ny = 2\n");
    found("array-of-tables", "[[fruit]]\nname = \"apple\"\n[[fruit]]\nname = \"pear\"\n", "fruit");
    // A header after `[[..]]` names the last table the array holds.
    found("into-the-last", "[[fruit]]\n[fruit.skin]\ncolour = \"red\"\n[[fruit]]\n[fruit.skin]\ncolour = \"green\"\n", "fruit");
    found("dotted-key", "[fruit]\napple.colour = \"red\"\n", "fruit.apple.colour");
    // Sub-tables may still be added below a table a dotted key made.
    found("under-a-dotted-key", "[fruit]\napple.colour = \"red\"\n[fruit.apple.texture]\nsmooth = true\n", "fruit.apple.texture.smooth");
    check("comments-and-blanks", "# one\n\n  # two\n[a] # three\nx = 1 # four\n");
    check("crlf", "a = 1\r\nb = 2\r\n");
    check("empty", "");

    // The refusals.
    check("table-twice", "[a]\n[a]\n");
    check("key-twice", "a = 1\na = 2\n");
    check("header-over-a-dotted-key", "[fruit]\napple.colour = \"red\"\n[fruit.apple]\n");
    check("extending-an-inline-table", "a = { b = 1 }\n[a.c]\n");
    check("table-over-an-array", "[[a]]\n[a]\n");
    check("appending-to-a-written-array", "a = [1]\n[[a]]\n");
    check("a-key-is-not-a-table", "[a]\nb = 1\n[a.b]\n");
    check("two-statements-on-one-line", "a = 1 b = 2\n");
    check("a-lone-carriage-return", "a = 1\rb = 2\n");
    check("an-unclosed-array", "a = [1, 2\n");
    check("an-unclosed-header", "[a\n");
    check("no-value", "a =\n");
    check("no-key", "= 1\n");
    check("a-trailing-comma-in-an-inline-table", "a = { b = 1, }\n");

    // The refusals a value makes.
    check("an-unknown-escape", "s = \"\\q\"\n");
    check("a-surrogate", "s = \"\\ud800\"\n");
    check("a-leading-zero", "n = 007\n");
    check("a-doubled-underscore", "n = 1__0\n");
    check("a-signed-hexadecimal", "n = -0xff\n");
    check("the-thirtieth-of-february", "d = 1979-02-30\n");
    check("an-hour-of-twenty-five", "d = 25:00:00\n");
    check("a-string-across-two-lines", "s = \"a\nb\"\n");
    return 0;
}
