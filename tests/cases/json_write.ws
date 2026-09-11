// Writing JSON, and reading back what was written.
//
// `std/der` has no writer on the grounds that nothing needed one. Something
// needs one here -- a program that reads an API almost always has to send one
// too -- and the reason to write it properly rather than to concatenate strings
// at the call site is that a body which cannot be read back by the reader
// beside it is a bug that waits for the first string with a quotation mark in
// it. So the round trip is the test, and it is taken twice: writing what was
// parsed and then parsing what was written must reach a fixed point.
//
// The comparison is of *rendered strings* rather than of values, because `==`
// on a struct holding an array is a compile error -- a `Value` carrying a
// `list.List[Value]` is not comparable, and saying so here is cheaper than
// discovering it.
// expect: scalars: [null,true,false,0,-1,1.5]
// expect: nesting: {"a":{"b":[1,[2,[3]]]}}
// expect: empty: {"a":[],"b":{}}
// expect: spacing-is-dropped: {"a":[1,2]}
// expect: order: {"z":1,"m":2,"a":3,"k9":4}
// expect: wide-object: held 1981
// expect: escapes: "\"\\/\b\f\n\r\t"
// expect: a-control-character: "a\u0001b"
// expect: a-nul: "\u0000"
// expect: above-the-basic-plane: "😀"
// expect: raw-utf8: "é中😀"
// expect: a-quote-in-a-name: {"a\"b":1}
// expect: an-empty-name: {"":1}
// expect: integral-float: 1.0
// expect: exponent: 1000.0
// expect: integer-stays-an-integer: 1
// expect: wide-integer: 9007199254740993
// expect: not-finite: [null,null,0.0]
// expect: pretty-2: {|.."a":.[|....1,|....{|......"b":.null|....}|..],|.."c":.[]|}
// expect: pretty-0: {|"a":.[|1,|{|"b":.null|}|],|"c":.[]|}
// expect: pretty-clamped: [|........1|]
// expect: pretty-reads-back: {"a":[1,{"b":null}],"c":[]}
// expect: nothing: null
const json = @import("std/json");
const list = @import("std/list");
const text = @import("std/str");

fn say(name: str, what: str) void {
    print(text.concat(name, text.concat(": ", what)));
    return;
}

/// Parse, write, parse, write. The two renderings must agree, which is what
/// says the writer produces something this reader takes and reads the same way.
fn round_trip(name: str, src: str) void {
    say(name, tripped(src));
    return;
}

/// The same check on a document too long to put in an expectation: the answer
/// is how many bytes held rather than which ones.
fn round_trip_quietly(name: str, src: str) void {
    const held = tripped(src);
    say(name, text.concat("held ", text.from_int(text.len(held))));
    return;
}

fn tripped(src: str) str {
    const first = json.parse(src);
    if (!first.ok) { return text.concat("failed -- ", first.message); }
    const written = json.write(first.root);
    const second = json.parse(written);
    if (!second.ok) { return text.concat("unreadable -- ", second.message); }
    if (!text.eq(written, json.write(second.root))) { return "drifted"; }
    return written;
}

/// A rendering on one line, with a newline as `|` and a space as `.`.
///
/// The harness trims leading spaces from an expectation and not from the
/// output, so an indented line cannot be expected at all -- which means the
/// indentation this is testing has to be made visible to be asserted on.
fn visible(s: str) str {
    return text.replace(text.replace(s, "\n", "|"), " ", ".");
}

fn main() i64 {
    // The compact form has no byte that is not content.
    round_trip("scalars", "[null, true, false, 0, -1, 1.5]");
    round_trip("nesting", "{\"a\": {\"b\": [1, [2, [3]]]}}");
    round_trip("empty", "{\"a\": [], \"b\": {}}");
    round_trip("spacing-is-dropped", " {  \"a\" :\n\t[ 1 , 2 ]  } ");
    // Order is the order the document had, not the order a hash would give.
    round_trip("order", "{\"z\": 1, \"m\": 2, \"a\": 3, \"k9\": 4}");
    // Two hundred members, so the object is past its index threshold and the
    // writer is walking the lists rather than anything the index decided.
    round_trip_quietly("wide-object", wide());

    // Escaping. `/` may be escaped and needs none, so it comes back bare; a
    // byte above 127 is already the UTF-8 it should be and passes through.
    round_trip("escapes", "\"\\\"\\\\\\/\\b\\f\\n\\r\\t\"");
    round_trip("a-control-character", "\"a\\u0001b\"");
    round_trip("a-nul", "\"\\u0000\"");
    round_trip("above-the-basic-plane", "\"\\ud83d\\ude00\"");
    round_trip("raw-utf8", "\"é中😀\"");
    round_trip("a-quote-in-a-name", "{\"a\\\"b\": 1}");
    round_trip("an-empty-name", "{\"\": 1}");

    // A float keeps its point, so it reads back as a float rather than as an
    // integer: `str.from_float` writes `1` for one and that is a different
    // shape coming back.
    round_trip("integral-float", "1.0");
    round_trip("exponent", "1e3");
    round_trip("integer-stays-an-integer", "1");
    round_trip("wide-integer", "9007199254740993");

    // The two numbers JSON cannot spell. An `f64` has them, a document has no
    // way to say them, and `null` is what `JSON.stringify` writes -- so this is
    // the one place a value does not survive being written, and it is stated
    // rather than discovered.
    var items: list.List[json.Value] = list.new();
    list.push(items, json.of_float(infinity()));
    list.push(items, json.of_float(not_a_number()));
    list.push(items, json.of_float(0.0));
    say("not-finite", json.write(json.of_array(items)));

    // The pretty form. `indent` is clamped, and zero is still a line per entry.
    const doc = json.parse("{\"a\": [1, {\"b\": null}], \"c\": []}");
    say("pretty-2", visible(json.write_pretty(doc.root, 2)));
    say("pretty-0", visible(json.write_pretty(doc.root, 0)));
    say("pretty-clamped", visible(json.write_pretty(json.parse("[1]").root, 99)));
    // What is written out indented is still what this reader takes.
    const again = json.parse(json.write_pretty(doc.root, 4));
    say("pretty-reads-back", json.write(again.root));

    // A value that is not there writes as `null`, which is the only thing a
    // document can say about one.
    say("nothing", json.write(json.parse("oops").root));
    return 0;
}

/// An object with two hundred members, written out by hand.
fn wide() str {
    const o = json.obj();
    var i = 0;
    while (i < 200) : (i += 1) {
        json.set(o, text.concat("k", text.from_int(i)), json.of_int(i));
    }
    return json.write(json.of_obj(o));
}

/// Made through the parser, because W# has no literal for either and `1.0/0.0`
/// is a way of saying it that depends on what the hardware does with a division.
fn infinity() f64 { return text.parse_float("inf") catch 0.0; }
fn not_a_number() f64 { return text.parse_float("nan") catch 0.0; }
