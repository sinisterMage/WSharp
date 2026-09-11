// JSON, read and written -- RFC 8259.
//
// The one format this project had named as a gap in its own documentation. The
// README said so about sharpie, which reads release tags over git's smart HTTP
// rather than through a forge's REST API "because that answers in JSON, W# has
// no JSON reader"; `--emit=api` answers in s-expressions and says the same
// thing from the other side. Everything needed to write it was already here --
// `str.parse_float` is a row `std/toml` put in the table -- so this module adds
// no builtins at all.
//
// **A value is a lattice, not a tagged struct.** The third use of the shape
// `std/x509`'s `SigKey` introduced and `std/toml`'s `Value` repeated: an empty
// supertype, one subtype per shape carrying its payload, and `as_int` an
// overload set whose base case says "this is not an integer". At three uses it
// is simply how this language reads a tagged format.
//
// **Parsing does not raise.** `parse` answers with a `Doc` -- a value, or a
// message and where the reader gave up -- because an error union carries a tag
// and nothing else, and `BadFormat` is not a thing to hand somebody holding a
// response they did not write. The first failure wins: a recursive-descent
// reader that has lost its place invents the rest.
//
// **Strict, in `std/der`'s sense.** The grammar and nothing beside it: no
// comments, no trailing commas, no `NaN`, no single quotes, no `+1` and no
// `01`. A reader that wants to be lenient can be; one that wants to be strict
// cannot recover leniency afterwards, and a document this accepts is one every
// other reader in the world accepts too.
//
// **Three things differ from `std/toml`, and each is because of where a JSON
// document comes from.** A manifest is a file somebody wrote and a JSON
// document arrives off a socket. So `Doc` carries a byte offset beside the
// line, because a minified response is one line and "line 1" of 40 KB says
// nothing; there is a nesting bound, because `[[[[` a hundred thousand deep is
// otherwise a stack overflow with no diagnostic; and the object keeps an index
// once it grows, because nothing promises how many members a stranger's object
// has.
const array = @import("std/array");
const bytes = @import("std/bytes");
const list = @import("std/list");
const map = @import("std/map");
const text = @import("std/str");

// ---------------------------------------------------------------------------
// The value lattice
// ---------------------------------------------------------------------------

/// Anything a JSON document can hold.
///
/// Empty, so it is also its own sole instance -- which is what a `Doc` that
/// failed puts where a value would be. That is *not* the same thing as the
/// document having said `null`, which is `Null` below: "there is nothing here"
/// and "it said there is nothing here" are different answers and a reader that
/// gave them the same shape would be throwing one away.
pub const Value = struct { };

pub const Null = struct : Value { };
pub const Str = struct : Value { s: str };
pub const Bool = struct : Value { b: bool };

/// JSON has one number type and W# has two, so the reader decides.
///
/// A lexeme with no point and no exponent that fits an `i64` is an `Int` and
/// everything else is a `Float`. Keeping the integer exact is the whole point:
/// an identifier out of an API is routinely a 64-bit number, and above 2^53 an
/// `f64` cannot hold one -- so a reader that answered only in floats would hand
/// back the wrong id with nothing to catch it.
pub const Int = struct : Value { n: i64 };
pub const Float = struct : Value { f: f64 };

pub const Arr = struct : Value { items: list.List[Value] };

/// An object: its members, in the order they were written, and an index once
/// there are enough of them to be worth one. See `index_of`.
pub const Obj = struct : Value {
    keys: list.List[str],
    vals: list.List[Value],
    index: ?map.Map[i64],
};

/// A fresh object with no members.
pub fn obj() Obj {
    return Obj{ .keys = list.new(), .vals = list.new(), .index = null };
}

// ---------------------------------------------------------------------------
// Making values
// ---------------------------------------------------------------------------
//
// Every constructor answers with a `Value` rather than with its own type,
// because a subtype does not coerce into a supertype at a call argument -- only
// at an annotated binding. One `const v: Value = ..` per shape, here, is what
// keeps that spelling out of every call site.

pub fn of_null() Value { const v: Value = Null{}; return v; }
pub fn of_str(s: str) Value { const v: Value = Str{ .s = s }; return v; }
pub fn of_int(n: i64) Value { const v: Value = Int{ .n = n }; return v; }
pub fn of_float(f: f64) Value { const v: Value = Float{ .f = f }; return v; }
pub fn of_bool(b: bool) Value { const v: Value = Bool{ .b = b }; return v; }
pub fn of_obj(o: Obj) Value { const v: Value = o; return v; }

pub fn of_array(items: list.List[Value]) Value {
    const v: Value = Arr{ .items = items };
    return v;
}

// ---------------------------------------------------------------------------
// Reading values back
// ---------------------------------------------------------------------------
//
// The downcast a language with no sum types has: an overload set over the
// lattice, whose base case is the failure. Every overload states the same
// error set, because a dispatched call has one type.

pub fn as_str(v: Value) !{BadFormat}str { return error.BadFormat; }
pub fn as_str(v: Str) !{BadFormat}str { return v.s; }

pub fn as_bool(v: Value) !{BadFormat}bool { return error.BadFormat; }
pub fn as_bool(v: Bool) !{BadFormat}bool { return v.b; }

/// An integer, and only an integer.
///
/// A `Float` is not one, even when it is whole: `1.0` is a number the document
/// chose to write with a point, and truncating quietly is precisely what a
/// strict reader exists to not do.
pub fn as_int(v: Value) !{BadFormat}i64 { return error.BadFormat; }
pub fn as_int(v: Int) !{BadFormat}i64 { return v.n; }

/// A number, either way round.
///
/// The `Int` overload is not symmetry for its own sake: JSON has one number
/// type, so a caller that wants a number should not have to ask which half of
/// ours the reader happened to land in.
pub fn as_float(v: Value) !{BadFormat}f64 { return error.BadFormat; }
pub fn as_float(v: Float) !{BadFormat}f64 { return v.f; }
pub fn as_float(v: Int) !{BadFormat}f64 { return f64(v.n); }

pub fn as_array(v: Value) !{BadFormat}list.List[Value] { return error.BadFormat; }
pub fn as_array(v: Arr) !{BadFormat}list.List[Value] { return v.items; }

pub fn as_obj(v: Value) !{BadFormat}Obj { return error.BadFormat; }
pub fn as_obj(v: Obj) !{BadFormat}Obj { return v; }

/// Whether the document said `null` -- which `as_*` cannot answer, because
/// `null` is not a payload to fail to be.
pub fn is_null(v: Value) bool { return false; }
pub fn is_null(v: Null) bool { return true; }

/// What shape a value is, for a caller that would rather ask than catch.
pub const KIND_NONE = 0;
pub const KIND_NULL = 1;
pub const KIND_STR = 2;
pub const KIND_INT = 3;
pub const KIND_FLOAT = 4;
pub const KIND_BOOL = 5;
pub const KIND_ARRAY = 6;
pub const KIND_OBJECT = 7;

pub fn kind(v: Value) i64 { return KIND_NONE; }
pub fn kind(v: Null) i64 { return KIND_NULL; }
pub fn kind(v: Str) i64 { return KIND_STR; }
pub fn kind(v: Int) i64 { return KIND_INT; }
pub fn kind(v: Float) i64 { return KIND_FLOAT; }
pub fn kind(v: Bool) i64 { return KIND_BOOL; }
pub fn kind(v: Arr) i64 { return KIND_ARRAY; }
pub fn kind(v: Obj) i64 { return KIND_OBJECT; }

// ---------------------------------------------------------------------------
// Objects
// ---------------------------------------------------------------------------
//
// Parallel key and value lists, because the order members were written in is
// the order they have to be written back out in and a hash table's order is
// neither that nor sorted. A linear scan over them is what `std/toml`'s table
// does, and what it says about it is that "if something ever puts a large table
// through this, the fix is a map and not a cleverer scan" -- with the reason it
// is fine there being that a manifest has tens of keys.
//
// A document off a network makes no such promise, so the scan is kept and a
// `map.Map[i64]` from key to position is built beside it the moment an object
// passes `INDEX_AT` members. A three-member object costs exactly what a table
// costs; a five-thousand-member one costs a hash. The threshold rather than an
// index from the first key is because the common object in a parsed document is
// small, and every allocation is a whole collection under `--gc-stress`.

/// How many members an object may have before it is worth indexing.
const INDEX_AT = 8;

fn index_of(o: Obj, key: str) i64 {
    if (o.index) |ix| { return map.get(ix, key) orelse -1; }
    // Hoisted, as every loop over a container here is: a builtin call is a
    // stack walk under `--gc-stress`, and `list.len` in the condition would be
    // one per member rather than one per lookup.
    const n = list.len(o.keys);
    var i = 0;
    while (i < n) : (i += 1) {
        if (text.eq(list.get(o.keys, i), key)) { return i; }
    }
    return -1;
}

fn build_index(o: Obj) void {
    const n = list.len(o.keys);
    const built: map.Map[i64] = map.with_capacity(n * 2);
    var i = 0;
    while (i < n) : (i += 1) { map.set(built, list.get(o.keys, i), i); }
    const held: ?map.Map[i64] = built;
    o.index = held;
    return;
}

pub fn get(o: Obj, key: str) ?Value {
    const at = index_of(o, key);
    if (at < 0) { return null; }
    return list.get(o.vals, at);
}

pub fn has(o: Obj, key: str) bool { return index_of(o, key) >= 0; }

/// The members this object holds, in the order they were written.
pub fn keys(o: Obj) []str { return list.to_array(o.keys); }

pub fn len(o: Obj) i64 { return list.len(o.keys); }

/// Add a member, or replace the one that is there.
///
/// Replacing keeps the position the name first had, which is what makes a
/// document with a repeated name round-trip to the same shape it arrived in.
pub fn set(o: Obj, key: str, v: Value) void {
    const at = index_of(o, key);
    if (at >= 0) { list.set(o.vals, at, v); return; }
    list.push(o.keys, key);
    list.push(o.vals, v);
    if (o.index) |ix| { map.set(ix, key, list.len(o.keys) - 1); return; }
    if (list.len(o.keys) > INDEX_AT) { build_index(o); }
    return;
}

/// The value at a dotted path, or null.
///
/// `lookup(doc.root, "data.user.name")` is what reading a response looks like.
/// It walks objects only: a component that was a number would be ambiguous with
/// a member *called* `0`, and JSON allows that name. `at` is how an array is
/// stepped into.
pub fn lookup(v: Value, path: str) ?Value {
    var here = v;
    const parts = text.split(path, ".");
    const n = array.len(parts);
    var i = 0;
    while (i < n) : (i += 1) {
        const o = as_obj(here) catch return null;
        here = get(o, parts[i]) orelse return null;
    }
    return here;
}

/// The `i`th element of an array, or null.
///
/// Null rather than the panic `list.get` would raise, because out of range is
/// the ordinary answer here: `list.get` is right for a container this program
/// built and wrong for one a stranger sent.
pub fn at(v: Value, i: i64) ?Value {
    const items = as_array(v) catch return null;
    if (i < 0 or i >= list.len(items)) { return null; }
    return list.get(items, i);
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// What `parse` answers with.
///
/// `ok` says which half to read. `line` is where the reader gave up, counting
/// from one, and `at` is the byte offset of the same place -- both, because a
/// document written for a person to read has useful lines and a minified one
/// has exactly one, and pointing at "line 1" of a 40 KB response helps nobody.
pub const Doc = struct { root: Value, ok: bool, message: str, line: i64, at: i64 };

/// How deeply arrays and objects may nest.
///
/// `std/toml` has no such bound because a manifest is a file somebody wrote.
/// This is the format that actually arrives from a socket, and the reader is
/// recursive, so without a bound a few hundred kilobytes of `[` is a stack
/// overflow -- a crash with no diagnostic -- rather than a document this
/// refuses. `std/x509`'s `MAX_CHAIN` is the same shape: a named bound, no knob.
pub const MAX_DEPTH = 128;

const P = struct {
    b: []u8,
    at: i64,
    n: i64,
    line: i64,
    depth: i64,
    ok: bool,
    message: str,
    fail_line: i64,
    fail_at: i64,
};

/// Read a whole JSON document.
///
/// One value, and then the end: RFC 8259 allows any value at the top level, so
/// a bare `42` is a document, and two of anything is not.
pub fn parse(src: str) Doc {
    const b = bytes.of(src);
    const p = P{
        .b = b,
        .at = 0,
        .n = array.len(b),
        .line = 1,
        .depth = 0,
        .ok = true,
        .message = "",
        .fail_line = 0,
        .fail_at = 0,
    };
    var root = Value{};
    refuse_bom(p);
    if (p.ok and p.n == 0) { fail(p, "the document is empty"); }
    if (p.ok) {
        skip_space(p);
        root = value(p);
    }
    if (p.ok) {
        skip_space(p);
        if (!at_end(p)) { fail(p, "there is more text after the end of the document"); }
    }
    return Doc{ .root = root, .ok = p.ok, .message = p.message, .line = p.fail_line, .at = p.fail_at };
}

/// A UTF-8 byte-order mark, refused by name.
///
/// RFC 8259 does not allow one. Saying which three bytes are in the way is the
/// difference between a minute and an afternoon, because a mark is invisible in
/// every editor that would be used to look for it -- and "expected a value" on
/// a file that looks perfectly fine is the least helpful thing a reader can
/// say.
fn refuse_bom(p: P) void {
    if (p.n >= 3 and i64(p.b[0]) == 239 and i64(p.b[1]) == 187 and i64(p.b[2]) == 191) {
        fail(p, "this document begins with a byte-order mark, which JSON does not allow");
    }
    return;
}

// ---- the cursor ------------------------------------------------------------

fn at_end(p: P) bool { return p.at >= p.n; }

/// The byte under the cursor, or -1 at the end.
///
/// A sentinel rather than a bounds check at every call, which is what lets
/// every test below be a plain comparison.
fn peek(p: P) i64 {
    if (p.at >= p.n) { return -1; }
    return i64(p.b[p.at]);
}

fn peek_at(p: P, k: i64) i64 {
    if (p.at + k >= p.n) { return -1; }
    return i64(p.b[p.at + k]);
}

fn bump(p: P) i64 {
    const c = peek(p);
    if (c >= 0) { p.at += 1; }
    if (c == LF) { p.line += 1; }
    return c;
}

/// Record the first failure and stop. Later ones are noise: a recursive-descent
/// reader that has lost its place invents them.
fn fail(p: P, m: str) void {
    if (p.ok) {
        p.ok = false;
        p.message = m;
        p.fail_line = p.line;
        p.fail_at = p.at;
    }
    return;
}

fn enter(p: P) void {
    p.depth += 1;
    if (p.depth > MAX_DEPTH) {
        fail(p, text.concat(text.concat("this document nests more than ",
            text.from_int(MAX_DEPTH)), " deep"));
    }
    return;
}

fn leave(p: P) void { p.depth -= 1; return; }

// ---- bytes -----------------------------------------------------------------
//
// W# has no character literal, so the alphabet is spelled out. The ones used
// once stand as numbers where the context says what they are.

const TAB = 9;
const LF = 10;
const CR = 13;
const SPACE = 32;
const QUOTE = 34;
const APOS = 39;
const PLUS = 43;
const COMMA = 44;
const MINUS = 45;
const DOT = 46;
const SLASH = 47;
const COLON = 58;
const LBRACKET = 91;
const BACKSLASH = 92;
const RBRACKET = 93;
const LBRACE = 123;
const RBRACE = 125;

fn is_digit(c: i64) bool { return c >= 48 and c <= 57; }

fn is_hex(c: i64) bool {
    return is_digit(c) or (c >= 97 and c <= 102) or (c >= 65 and c <= 70);
}

fn is_alpha(c: i64) bool {
    return (c >= 97 and c <= 122) or (c >= 65 and c <= 90);
}

fn hex_value(c: i64) i64 {
    if (c >= 48 and c <= 57) { return c - 48; }
    if (c >= 97 and c <= 102) { return c - 87; }
    return c - 55;
}

/// The four bytes JSON calls whitespace, and no others.
///
/// A form feed is not one and neither is a vertical tab, which is the sort of
/// thing a reader built on somebody else's `is_space` gets wrong.
fn skip_space(p: P) void {
    var going = true;
    while (going) {
        const c = peek(p);
        if (c == SPACE or c == TAB or c == CR) { p.at += 1; continue; }
        if (c == LF) { bump(p); continue; }
        going = false;
    }
    return;
}

/// `w` under the cursor, consumed if it is there.
fn take_word(p: P, w: str) bool {
    const n = text.len(w);
    var i = 0;
    while (i < n) : (i += 1) {
        if (peek_at(p, i) != text.byte_at(w, i)) { return false; }
    }
    p.at += n;
    return true;
}

// ---- values ----------------------------------------------------------------

fn value(p: P) Value {
    const c = peek(p);
    if (c == QUOTE) { return of_str(string(p)); }
    if (c == LBRACKET) { return array_value(p); }
    if (c == LBRACE) { return object_value(p); }
    if (c == MINUS or is_digit(c)) { return number(p); }
    if (c == 116 and take_word(p, "true")) { return of_bool(true); }
    if (c == 102 and take_word(p, "false")) { return of_bool(false); }
    if (c == 110 and take_word(p, "null")) { return of_null(); }
    if (c < 0) { fail(p, "expected a value, and the document ends here"); return Value{}; }
    // The three mistakes worth naming, because each is a thing a person who has
    // written JavaScript or Python will reasonably have typed.
    if (c == APOS) { fail(p, "JSON has no single-quoted string"); return Value{}; }
    if (c == PLUS) { fail(p, "a number may not begin with `+`"); return Value{}; }
    if (c == DOT) { fail(p, "a number needs a digit in front of its decimal point"); return Value{}; }
    if (is_alpha(c)) {
        fail(p, "expected a value: the only bare words JSON has are `true`, `false` and `null`");
        return Value{};
    }
    fail(p, "expected a value");
    return Value{};
}

fn array_value(p: P) Value {
    p.at += 1;
    enter(p);
    if (!p.ok) { return Value{}; }
    var items: list.List[Value] = list.new();
    skip_space(p);
    if (peek(p) == RBRACKET) { p.at += 1; leave(p); return of_array(items); }
    var going = true;
    while (going and p.ok) {
        skip_space(p);
        list.push(items, value(p));
        if (!p.ok) { return Value{}; }
        skip_space(p);
        const c = peek(p);
        if (c == COMMA) {
            p.at += 1;
            skip_space(p);
            // JSON has no trailing comma, and saying so is friendlier than the
            // "expected a value" it would otherwise be.
            if (peek(p) == RBRACKET) { fail(p, "an array cannot end with a comma"); return Value{}; }
            continue;
        }
        if (c == RBRACKET) { p.at += 1; going = false; continue; }
        if (c < 0) { fail(p, "expected `]` to close this array"); return Value{}; }
        fail(p, "expected `,` or `]` in an array");
    }
    leave(p);
    return of_array(items);
}

/// `{ "k": v, .. }`.
///
/// A repeated name replaces what was there, which is what JavaScript's reader
/// does. RFC 8259 says only that the behaviour is unpredictable when names are
/// repeated, so there is no right answer to be had -- only a documented one.
fn object_value(p: P) Value {
    p.at += 1;
    enter(p);
    if (!p.ok) { return Value{}; }
    const o = obj();
    skip_space(p);
    if (peek(p) == RBRACE) { p.at += 1; leave(p); return of_obj(o); }
    var going = true;
    while (going and p.ok) {
        skip_space(p);
        if (peek(p) != QUOTE) { fail(p, "a member's name must be a quoted string"); return Value{}; }
        const key = string(p);
        if (!p.ok) { return Value{}; }
        skip_space(p);
        if (peek(p) != COLON) { fail(p, "expected `:` after a member's name"); return Value{}; }
        p.at += 1;
        skip_space(p);
        const v = value(p);
        if (!p.ok) { return Value{}; }
        set(o, key, v);
        skip_space(p);
        const c = peek(p);
        if (c == COMMA) {
            p.at += 1;
            skip_space(p);
            if (peek(p) == RBRACE) { fail(p, "an object cannot end with a comma"); return Value{}; }
            continue;
        }
        if (c == RBRACE) { p.at += 1; going = false; continue; }
        if (c < 0) { fail(p, "expected `}` to close this object"); return Value{}; }
        fail(p, "expected `,` or `}` in an object");
    }
    leave(p);
    return of_obj(o);
}

// ---- strings ---------------------------------------------------------------

fn string(p: P) str {
    p.at += 1;
    const b = bytes.buf(32);
    var going = true;
    while (going and p.ok) {
        const c = peek(p);
        if (c < 0) { fail(p, "a string is not closed before the end of the document"); return ""; }
        if (c == QUOTE) { p.at += 1; going = false; continue; }
        if (c == BACKSLASH) { p.at += 1; escape(p, b); continue; }
        // Everything below space, including a raw newline, which is why a
        // string that runs off its line is caught here rather than by counting
        // lines. A byte above 127 passes through: see `write_string`.
        if (c < 32) {
            fail(p, "a string cannot hold a raw control character; write it as an escape");
            return "";
        }
        bytes.put_u8(b, c);
        p.at += 1;
    }
    return bytes.to_str(bytes.taken(b));
}

/// What follows a backslash. The backslash itself is already consumed.
fn escape(p: P, b: bytes.Buf) void {
    const c = peek(p);
    if (c < 0) { fail(p, "a string is not closed before the end of the document"); return; }
    p.at += 1;
    if (c == QUOTE) { bytes.put_u8(b, QUOTE); return; }
    if (c == BACKSLASH) { bytes.put_u8(b, BACKSLASH); return; }
    if (c == SLASH) { bytes.put_u8(b, SLASH); return; }
    if (c == 98) { bytes.put_u8(b, 8); return; }
    if (c == 102) { bytes.put_u8(b, 12); return; }
    if (c == 110) { bytes.put_u8(b, LF); return; }
    if (c == 114) { bytes.put_u8(b, CR); return; }
    if (c == 116) { bytes.put_u8(b, TAB); return; }
    if (c == 117) { unicode(p, b); return; }
    fail(p, "unknown escape sequence: the ones JSON has are \\\" \\\\ \\/ \\b \\f \\n \\r \\t \\uXXXX");
    return;
}

/// `\uXXXX`, and the surrogate pair it may be half of.
///
/// The one place JSON asks for more than TOML does. `\UXXXXXXXX` does not
/// exist here, so a code point above the basic plane is written as *two*
/// escapes -- a high surrogate and a low one -- and only together do they name
/// a character. Each half on its own is a UTF-16 code unit that came from an
/// encoder which did not finish the job, and it is refused rather than encoded,
/// because `\uD800` has no UTF-8 spelling to encode it into.
fn unicode(p: P, b: bytes.Buf) void {
    const first = hex4(p);
    if (!p.ok) { return; }
    if (first >= 56320 and first <= 57343) {
        fail(p, "this is a low surrogate with no high surrogate in front of it");
        return;
    }
    if (first < 55296 or first > 56319) { bytes.put_utf8(b, first); return; }
    if (peek(p) != BACKSLASH or peek_at(p, 1) != 117) {
        fail(p, "a high surrogate must be followed by a low surrogate");
        return;
    }
    p.at += 2;
    const second = hex4(p);
    if (!p.ok) { return; }
    if (second < 56320 or second > 57343) {
        fail(p, "a high surrogate must be followed by a low surrogate");
        return;
    }
    bytes.put_utf8(b, 65536 + ((first - 55296) << 10) + (second - 56320));
    return;
}

fn hex4(p: P) i64 {
    var cp = 0;
    var i = 0;
    while (i < 4) : (i += 1) {
        const c = peek(p);
        if (!is_hex(c)) { fail(p, "an escape needs four hexadecimal digits after its `u`"); return 0; }
        cp = cp * 16 + hex_value(c);
        p.at += 1;
    }
    return cp;
}

// ---- numbers ---------------------------------------------------------------

/// A number, lexed by the grammar and then converted.
///
/// The shape is checked here and only the text is handed on, because JSON is
/// stricter than either conversion: `str.parse_int` would take `+1` and
/// `str.parse_float` would take `.5`, `5.` and `nan`, none of which is a JSON
/// number. The same division of labour `std/toml` uses, and the reason
/// `str.parse_float` is in the builtin table at all.
fn number(p: P) Value {
    const start = p.at;
    if (peek(p) == MINUS) { p.at += 1; }
    const whole = p.at;
    if (!take_digits(p)) { fail(p, "a number needs at least one digit"); return Value{}; }
    // `0` is the only integer part that may begin with a zero, so `007` is not
    // seven and `-01` is not minus one.
    if (p.at - whole > 1 and i64(p.b[whole]) == 48) {
        fail(p, "a number may not have a leading zero");
        return Value{};
    }
    var floating = false;
    if (peek(p) == DOT) {
        floating = true;
        p.at += 1;
        if (!take_digits(p)) {
            fail(p, "a number needs at least one digit after its decimal point");
            return Value{};
        }
    }
    const e = peek(p);
    if (e == 101 or e == 69) {
        floating = true;
        p.at += 1;
        if (peek(p) == PLUS or peek(p) == MINUS) { p.at += 1; }
        if (!take_digits(p)) {
            fail(p, "a number needs at least one digit after its exponent");
            return Value{};
        }
    }
    const raw = bytes.slice_str(p.b, start, p.at);
    if (floating) { return of_float(as_double(p, raw)); }
    // An integer too wide for an `i64` becomes a float rather than a refusal:
    // JSON has one number type and says nothing about how wide it is, so a
    // reader that rejected one would be rejecting a valid document.
    const n = text.parse_int(raw) catch return of_float(as_double(p, raw));
    return of_int(n);
}

fn take_digits(p: P) bool {
    const from = p.at;
    while (is_digit(peek(p))) { p.at += 1; }
    return p.at > from;
}

/// The `f64` nearest what the text names.
///
/// A magnitude too large for an `f64` becomes an infinity rather than a
/// failure, which is what every other reader answers and what the writer then
/// turns back into `null`. The failure arm is for text this machine cannot read
/// at all, which the grammar above should already have refused.
fn as_double(p: P, raw: str) f64 {
    return text.parse_float(raw) catch {
        fail(p, "this is not a number this machine can hold");
        0.0
    };
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------
//
// `std/der` has no writer, on the grounds that nothing needed one. Something
// needs one here: a program that reads an API almost always has to send one
// too, and the reason to write it properly rather than to concatenate strings
// at the call site is that a body which cannot be read back by the reader
// beside it is a bug that waits for the first string with a quotation mark in
// it. The round trip is the test.

/// Render a value as JSON on one line, with no space that is not content.
pub fn write(v: Value) str {
    const b = bytes.buf(256);
    write_value(b, v, -1, 0);
    return bytes.to_str(bytes.taken(b));
}

/// Render a value as JSON, one member or element to a line.
///
/// `indent` is how many spaces a level costs, clamped to 0..8 -- zero still
/// being a line per entry, flush left, which is what a diff wants and `write`
/// is not.
pub fn write_pretty(v: Value, indent: i64) str {
    var step = indent;
    if (step < 0) { step = 0; }
    if (step > 8) { step = 8; }
    const b = bytes.buf(256);
    write_value(b, v, step, 0);
    return bytes.to_str(bytes.taken(b));
}

/// One value into `b`. `step` below zero is the compact form; `level` is how
/// far in this value sits.
fn write_value(b: bytes.Buf, v: Value, step: i64, level: i64) void {
    const k = kind(v);
    if (k == KIND_STR) { write_string(b, as_str(v) catch ""); return; }
    if (k == KIND_INT) { bytes.put_str(b, text.from_int(as_int(v) catch 0)); return; }
    if (k == KIND_FLOAT) { bytes.put_str(b, float_text(as_float(v) catch 0.0)); return; }
    if (k == KIND_BOOL) {
        if (as_bool(v) catch false) { bytes.put_str(b, "true"); } else { bytes.put_str(b, "false"); }
        return;
    }
    if (k == KIND_ARRAY) { write_array(b, v, step, level); return; }
    if (k == KIND_OBJECT) { write_object(b, v, step, level); return; }
    // `Null`, and also the bare `Value` a failed parse leaves behind: there is
    // nothing else a document can say about a value that is not there.
    bytes.put_str(b, "null");
    return;
}

fn write_array(b: bytes.Buf, v: Value, step: i64, level: i64) void {
    var empty: list.List[Value] = list.new();
    const items = as_array(v) catch empty;
    const n = list.len(items);
    // An empty array is `[]` in both forms: a line break around nothing is not
    // an aid to reading it.
    if (n == 0) { bytes.put_str(b, "[]"); return; }
    bytes.put_u8(b, LBRACKET);
    var i = 0;
    while (i < n) : (i += 1) {
        if (i > 0) { bytes.put_u8(b, COMMA); }
        newline(b, step, level + 1);
        write_value(b, list.get(items, i), step, level + 1);
    }
    newline(b, step, level);
    bytes.put_u8(b, RBRACKET);
    return;
}

fn write_object(b: bytes.Buf, v: Value, step: i64, level: i64) void {
    const o = as_obj(v) catch obj();
    const n = list.len(o.keys);
    if (n == 0) { bytes.put_str(b, "{}"); return; }
    bytes.put_u8(b, LBRACE);
    var i = 0;
    while (i < n) : (i += 1) {
        if (i > 0) { bytes.put_u8(b, COMMA); }
        newline(b, step, level + 1);
        write_string(b, list.get(o.keys, i));
        bytes.put_u8(b, COLON);
        if (step >= 0) { bytes.put_u8(b, SPACE); }
        write_value(b, list.get(o.vals, i), step, level + 1);
    }
    newline(b, step, level);
    bytes.put_u8(b, RBRACE);
    return;
}

/// A line break and this level's indentation -- or nothing at all, which is
/// what makes the compact writer and the pretty one the same walk.
fn newline(b: bytes.Buf, step: i64, level: i64) void {
    if (step < 0) { return; }
    bytes.put_u8(b, LF);
    const n = step * level;
    var i = 0;
    while (i < n) : (i += 1) { bytes.put_u8(b, SPACE); }
    return;
}

/// A string, escaped.
///
/// `"`, `\` and the control characters, and nothing else. `/` may be escaped
/// and needs no escaping, so it is not. A byte above 127 is already the UTF-8
/// it should be written as: turning one into `\uXXXX` would mean decoding what
/// the program was handed and re-encoding it, and a W# `str` is bytes rather
/// than text -- so the reader that gets this back sees exactly the bytes that
/// went in.
fn write_string(b: bytes.Buf, s: str) void {
    bytes.put_u8(b, QUOTE);
    const n = text.len(s);
    var i = 0;
    while (i < n) : (i += 1) {
        const c = text.byte_at(s, i);
        if (c == QUOTE or c == BACKSLASH) { bytes.put_u8(b, BACKSLASH); bytes.put_u8(b, c); continue; }
        if (c == 8) { bytes.put_str(b, "\\b"); continue; }
        if (c == 12) { bytes.put_str(b, "\\f"); continue; }
        if (c == LF) { bytes.put_str(b, "\\n"); continue; }
        if (c == CR) { bytes.put_str(b, "\\r"); continue; }
        if (c == TAB) { bytes.put_str(b, "\\t"); continue; }
        if (c < 32) { bytes.put_str(b, escape_hex(c)); continue; }
        bytes.put_u8(b, c);
    }
    bytes.put_u8(b, QUOTE);
    return;
}

fn escape_hex(c: i64) str {
    const digits = "0123456789abcdef";
    var out = "\\u00";
    out = text.concat(out, text.from_byte(text.byte_at(digits, (c >> 4) & 15)));
    return text.concat(out, text.from_byte(text.byte_at(digits, c & 15)));
}

/// A float as JSON spells it.
///
/// Two fixes over `str.from_float`. It writes `1` for one, which is valid JSON
/// and reads back as an *integer* -- so a lexeme with neither a point nor an
/// exponent gets `.0`, and a `Float` survives the round trip as a `Float`. And
/// it writes `inf` and `NaN`, which JSON has no spelling for at all; those
/// become `null`, which is what `JSON.stringify` does and the only answer that
/// is still a JSON document.
fn float_text(f: f64) str {
    const s = text.from_float(f);
    const n = text.len(s);
    var i = 0;
    while (i < n) : (i += 1) {
        const c = text.byte_at(s, i);
        if (c == DOT or c == 101 or c == 69) { return s; }
        // Any other letter means `inf`, `-inf` or `NaN`.
        if (!is_digit(c) and c != MINUS and c != PLUS) { return "null"; }
    }
    return text.concat(s, ".0");
}
