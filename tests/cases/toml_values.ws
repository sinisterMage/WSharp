// TOML 1.0's value grammar: all four string forms, four integer bases, floats,
// and the four date and time forms. A value is a lattice rather than a tagged
// struct -- `Value` is an empty supertype and `as_int` is an overload set whose
// base case says "this is not an integer", which is the downcast a language
// with no sum types has.
// expect: basic: str a	b"c
// expect: multiline: str the quick brown fox
// expect: inner-quotes: str a""b
// expect: closing-quote: str abc"
// expect: literal: str C:\Users\n
// expect: multiline-literal: str hi
// expect: escaped-code-points: str é😀
// expect: decimal: int 1000000
// expect: hex: int 3735928559
// expect: octal: int 493
// expect: binary: int 10
// expect: negative: int -17
// expect: float: float -0.0015
// expect: float-zero: float 0
// expect: bool: bool true
// expect: offset-date-time: date 1979-05-27T00:32:00.999999-07:00
// expect: local-date-time: date 1979-05-27 07:32:00
// expect: local-date: date 1979-05-27
// expect: local-time: date 07:32:00.999
// expect: leap-second: date 1990-12-31T23:59:60Z
// expect: leap-day: date 2024-02-29
// expect: array: array 3
// expect: array-multiline: array 2
// expect: array-nested: array 3
// expect: array-empty: array 0
// expect: inline-table: table
// expect: inline-table-value: str two
// expect: inline-table-empty: table
// expect: quoted-key-via-lookup: missing
// expect: quoted-key-via-get: int 1
const list = @import("std/list");
const text = @import("std/str");
const toml = @import("std/toml");

/// Parse `src`, look `path` up, and print what kind of thing is there and what
/// it says. One line per case, so the expectations read as a table.
fn show(name: str, src: str, path: str) void {
    const doc = toml.parse(src);
    if (!doc.ok) {
        print(text.concat(name, text.concat(": failed -- ", doc.message)));
        return;
    }
    const v = toml.lookup(doc.root, path) orelse {
        print(text.concat(name, ": missing"));
        return;
    };
    print(text.concat(name, text.concat(": ", render(v))));
    return;
}

fn render(v: toml.Value) str {
    const k = toml.kind(v);
    if (k == toml.KIND_STR) { return text.concat("str ", toml.as_str(v) catch ""); }
    if (k == toml.KIND_INT) { return text.concat("int ", text.from_int(toml.as_int(v) catch 0)); }
    if (k == toml.KIND_FLOAT) { return text.concat("float ", text.from_float(toml.as_float(v) catch 0.0)); }
    if (k == toml.KIND_BOOL) { return text.concat("bool ", yes_no(toml.as_bool(v) catch false)); }
    if (k == toml.KIND_DATE) { return text.concat("date ", toml.as_date(v) catch ""); }
    if (k == toml.KIND_ARRAY) {
        var empty: list.List[toml.Value] = list.new();
        return text.concat("array ", text.from_int(list.len(toml.as_array(v) catch empty)));
    }
    if (k == toml.KIND_TABLE) { return "table"; }
    return "?";
}

fn yes_no(b: bool) str { if (b) { return "true"; } return "false"; }

fn main() i64 {
    show("basic", "s = \"a\\tb\\\"c\"\n", "s");
    // A newline straight after the opening delimiter is not part of the value,
    // and a backslash at the end of a line swallows the indentation after it.
    show("multiline", "s = \"\"\"\nthe quick \\\n     brown fox\"\"\"\n", "s");
    // One or two quotes may stand inside unescaped, so the close is found by
    // counting a run rather than by matching three.
    show("inner-quotes", "s = \"\"\"a\"\"b\"\"\"\n", "s");
    show("closing-quote", "s = \"\"\"abc\"\"\"\"\n", "s");
    show("literal", "s = 'C:\\Users\\n'\n", "s");
    show("multiline-literal", "s = '''\nhi'''\n", "s");
    show("escaped-code-points", "s = \"\\u00e9\\U0001F600\"\n", "s");

    show("decimal", "n = 1_000_000\n", "n");
    show("hex", "n = 0xDEAD_beef\n", "n");
    show("octal", "n = 0o755\n", "n");
    show("binary", "n = 0b1010\n", "n");
    show("negative", "n = -17\n", "n");
    show("float", "n = -1.5e-3\n", "n");
    show("float-zero", "n = 0.0\n", "n");

    show("bool", "b = true\n", "b");

    show("offset-date-time", "d = 1979-05-27T00:32:00.999999-07:00\n", "d");
    // A local date-time may be joined by a space, which is the one reason the
    // scalar lexer has to look past whitespace.
    show("local-date-time", "d = 1979-05-27 07:32:00\n", "d");
    show("local-date", "d = 1979-05-27\n", "d");
    show("local-time", "d = 07:32:00.999\n", "d");
    show("leap-second", "d = 1990-12-31T23:59:60Z\n", "d");
    show("leap-day", "d = 2024-02-29\n", "d");

    show("array", "a = [1, 2, 3]\n", "a");
    show("array-multiline", "a = [\n 1,\n 2,\n]\n", "a");
    show("array-nested", "a = [[1, 2], [\"x\"], []]\n", "a");
    show("array-empty", "a = []\n", "a");
    show("inline-table", "a = { b = 1, c = \"two\" }\n", "a");
    show("inline-table-value", "a = { b = 1, c = \"two\" }\n", "a.c");
    show("inline-table-empty", "a = {}\n", "a");

    // `lookup` splits on `.`, so a key that holds one is reached with `get`.
    const doc = toml.parse("\"a.b\" = 1\n");
    show("quoted-key-via-lookup", "\"a.b\" = 1\n", "a.b");
    print(if (toml.get(doc.root, "a.b")) |v| text.concat("quoted-key-via-get: ", render(v)) else "quoted-key-via-get: missing");
    return 0;
}
