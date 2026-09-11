// JSON's value grammar -- RFC 8259. Every scalar, every escape, the surrogate
// pair that is the one thing JSON asks for and TOML does not, and the two ways
// into a structure.
//
// A value is a lattice rather than a tagged struct, so `render` asks `kind` and
// the `as_*` calls below it cannot be reached at the wrong shape.
//
// Every accept and reject in this file was confirmed against `node`'s
// `JSON.parse` and `python3`'s `json.loads` before it was written down.
// expect: top-level-string: str hi
// expect: top-level-int: int 42
// expect: top-level-bool: bool true
// expect: top-level-null: null
// expect: top-level-array: array 3
// expect: top-level-object: object 1
// expect: empty-array: array 0
// expect: empty-object: object 0
// expect: integer: int -17
// expect: exact-big-integer: int 9007199254740993
// expect: fraction: float -0.0015
// expect: exponent: float 1000
// expect: exponent-signed: float 1000
// expect: exponent-negative: float 0.0015
// expect: zero: int 0
// expect: zero-point: float 0
// expect: integer-too-wide: float 123456789012345680000000000000
// expect: escapes: [225c2f080c0a0d09]
// expect: escaped-code-points: str Aé中
// expect: surrogate-pair: str 😀
// expect: raw-utf8: str é😀
// expect: escaped-quote-in-key: object 1
// expect: empty-string: []
// expect: escaped-nul: [610062]
// expect: spaced: object 1
// expect: nested: str deep
// expect: through-object: array 2
// expect: missing: missing
// expect: array-in-the-path: missing
// expect: duplicate-name: int 2
// expect: empty-name: int 7
// expect: at-0: int 10
// expect: at-1: array 2
// expect: at-nested: int 30
// expect: at-past-the-end: missing
// expect: at-negative: missing
// expect: at-not-an-array: missing
// expect: as_float-of-int: 1
// expect: as_float-of-float: 1
// expect: as_int-of-int: 1
// expect: as_int-of-float: -1
// expect: is_null-of-null: true
// expect: is_null-of-zero: false
const bytes = @import("std/bytes");
const json = @import("std/json");
const list = @import("std/list");
const text = @import("std/str");

/// Parse `src` and say what the whole document is.
fn root(name: str, src: str) void {
    const doc = json.parse(src);
    if (!doc.ok) { print(text.concat(name, text.concat(": failed -- ", doc.message))); return; }
    print(text.concat(name, text.concat(": ", render(doc.root))));
    return;
}

/// Parse `src`, walk `path`, and say what is there.
fn show(name: str, src: str, path: str) void {
    const doc = json.parse(src);
    if (!doc.ok) { print(text.concat(name, text.concat(": failed -- ", doc.message))); return; }
    const v = json.lookup(doc.root, path) orelse {
        print(text.concat(name, ": missing"));
        return;
    };
    print(text.concat(name, text.concat(": ", render(v))));
    return;
}

fn render(v: json.Value) str {
    const k = json.kind(v);
    if (k == json.KIND_NULL) { return "null"; }
    if (k == json.KIND_STR) { return text.concat("str ", json.as_str(v) catch ""); }
    if (k == json.KIND_INT) { return text.concat("int ", text.from_int(json.as_int(v) catch 0)); }
    if (k == json.KIND_FLOAT) { return text.concat("float ", text.from_float(json.as_float(v) catch 0.0)); }
    if (k == json.KIND_BOOL) { return text.concat("bool ", yes_no(json.as_bool(v) catch false)); }
    if (k == json.KIND_ARRAY) {
        var empty: list.List[json.Value] = list.new();
        return text.concat("array ", text.from_int(list.len(json.as_array(v) catch empty)));
    }
    if (k == json.KIND_OBJECT) {
        return text.concat("object ", text.from_int(json.len(json.as_obj(v) catch json.obj())));
    }
    return "?";
}

/// The bytes a string decodes to, as hex in brackets.
///
/// Five of the eight escapes decode to control characters and one of them is a
/// newline, so a case cannot expect the decoded text on a line: it would be
/// several lines, two of which the harness trims. The brackets are so that the
/// empty string is `[]` rather than a line ending in a space.
fn decoded(name: str, src: str) void {
    const doc = json.parse(src);
    if (!doc.ok) { print(text.concat(name, text.concat(": failed -- ", doc.message))); return; }
    const hex = bytes.to_hex(bytes.of(json.as_str(doc.root) catch ""));
    print(text.concat(name, text.concat(": [", text.concat(hex, "]"))));
    return;
}

fn yes_no(b: bool) str { if (b) { return "true"; } return "false"; }

fn main() i64 {
    // Any value is a document. TOML's root is always a table; this one's is not.
    root("top-level-string", "\"hi\"");
    root("top-level-int", "42");
    root("top-level-bool", "true");
    root("top-level-null", "null");
    root("top-level-array", "[1, 2, 3]");
    root("top-level-object", "{\"a\": 1}");
    root("empty-array", "[]");
    root("empty-object", "{}");

    // Numbers. A lexeme with no point and no exponent is an integer, which is
    // what keeps a 64-bit identifier exact: `9007199254740993` is a number an
    // `f64` cannot hold, and JavaScript reads it back one short.
    root("integer", "-17");
    root("exact-big-integer", "9007199254740993");
    root("fraction", "-0.0015");
    root("exponent", "1e3");
    root("exponent-signed", "1E+3");
    root("exponent-negative", "1.5e-3");
    root("zero", "0");
    root("zero-point", "0.0");
    // Too wide for an `i64`, so it becomes a float rather than a refusal: JSON
    // has one number type and says nothing about how wide it is.
    root("integer-too-wide", "123456789012345678901234567890");

    // Strings. The eight escapes, and `/` which may be escaped and need not be.
    decoded("escapes", "\"\\\"\\\\\\/\\b\\f\\n\\r\\t\"");
    root("escaped-code-points", "\"\\u0041\\u00e9\\u4e2d\"");
    // Above the basic plane JSON writes two escapes and only the pair names a
    // character. This is the one thing it asks for that TOML does not.
    root("surrogate-pair", "\"\\ud83d\\ude00\"");
    // A byte above 127 is already the UTF-8 it should be, and passes through.
    root("raw-utf8", "\"é😀\"");
    root("escaped-quote-in-key", "{\"a\\\"b\": 1}");
    decoded("empty-string", "\"\"");
    decoded("escaped-nul", "\"a\\u0000b\"");

    // Whitespace is the four bytes and no others.
    root("spaced", " {\n\t\"a\" : [ 1 , 2 ]\r\n} ");

    // Structure.
    show("nested", "{\"a\": {\"b\": {\"c\": \"deep\"}}}", "a.b.c");
    show("through-object", "{\"a\": [1, 2]}", "a");
    show("missing", "{\"a\": 1}", "b");
    // `lookup` walks objects only, so an array in the middle is not a path.
    show("array-in-the-path", "{\"a\": [{\"b\": 1}]}", "a.b");
    // A repeated name replaces what was there, as JavaScript's reader does.
    show("duplicate-name", "{\"a\": 1, \"a\": 2}", "a");
    show("empty-name", "{\"\": 7}", "");

    // An array is stepped into with `at`, which answers null out of range
    // rather than panicking as `list.get` would.
    const doc = json.parse("[10, [20, 30]]");
    print(rendered("at-0", json.at(doc.root, 0)));
    print(rendered("at-1", json.at(doc.root, 1)));
    print(rendered("at-nested", json.at(json.at(doc.root, 1) orelse json.of_null(), 1)));
    print(rendered("at-past-the-end", json.at(doc.root, 2)));
    print(rendered("at-negative", json.at(doc.root, -1)));
    print(rendered("at-not-an-array", json.at(json.of_int(1), 0)));

    // `as_float` takes either half of the number type; `as_int` takes only an
    // integer, because `1.0` is a number the document chose to write with a
    // point and truncating quietly is what a strict reader is for.
    const one = json.parse("1").root;
    const one_point_oh = json.parse("1.0").root;
    say("as_float-of-int", text.from_float(json.as_float(one) catch -1.0));
    say("as_float-of-float", text.from_float(json.as_float(one_point_oh) catch -1.0));
    say("as_int-of-int", text.from_int(json.as_int(one) catch -1));
    say("as_int-of-float", text.from_int(json.as_int(one_point_oh) catch -1));

    // `null` is a value the document said, which is not the same as there being
    // no value -- so it is a shape of its own rather than the bare supertype.
    say("is_null-of-null", yes_no(json.is_null(json.parse("null").root)));
    say("is_null-of-zero", yes_no(json.is_null(json.parse("0").root)));
    return 0;
}

fn rendered(name: str, v: ?json.Value) str {
    const found = v orelse return text.concat(name, ": missing");
    return text.concat(name, text.concat(": ", render(found)));
}

fn say(name: str, what: str) void {
    print(text.concat(name, text.concat(": ", what)));
    return;
}
