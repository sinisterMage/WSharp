// TOML 1.0.0, whole.
//
// Written because item 11's manifest and lockfile are TOML and a *subset* of
// TOML is a promise the file extension makes and the code does not keep: a
// reader that accepts three quarters of the grammar produces "syntax error" on
// a file every other tool in the world reads, and the person holding it has no
// way to tell which quarter they landed in. So: dotted keys, all four string
// forms, integers in four bases, floats with exponents, the four date and time
// forms, arrays, inline tables, `[table]`, `[[array of tables]]`, and the
// redefinition rules that are the part nobody expects to be hard.
//
// **A value is a lattice, not a tagged struct.** W# has no sum types, and the
// thing it has instead is a nominal subtype lattice with multiple dispatch --
// so `Value` is an empty supertype, each shape is a subtype carrying its
// payload, and `as_int` is an overload set whose base case says "this is not an
// integer". `std/x509`'s `SigKey` is the original of this shape; a tagged
// struct with a `kind` field and seven mostly-null payload fields would be the
// same information with the checking taken out.
//
// **Parsing does not raise.** An error union carries a tag and nothing else,
// and "BadFormat" is not a thing to hand somebody holding a 200-line manifest.
// `parse` answers with a `Doc`, which is either a table or a message and the
// line it happened on. The tool's whole job is to be good on the day it goes
// wrong, and this is the first place that shows.
const array = @import("std/array");
const bytes = @import("std/bytes");
const list = @import("std/list");
const text = @import("std/str");

// ---------------------------------------------------------------------------
// The value lattice
// ---------------------------------------------------------------------------

/// Anything a TOML file can hold.
///
/// Empty, so it is also its own sole instance -- which is what a `Doc` that
/// failed puts where a value would be.
pub const Value = struct { };

pub const Str = struct : Value { s: str };
pub const Int = struct : Value { n: i64 };
pub const Float = struct : Value { f: f64 };
pub const Bool = struct : Value { b: bool };

/// A date, a time, or both, kept as it was written.
///
/// TOML's four date-and-time forms are validated here and *not* converted:
/// nothing in a manifest does calendar arithmetic, `std/time` is one function
/// answering with seconds since the epoch, and a value that has been through a
/// conversion nobody needed is a value that can be wrong in a new way. The
/// shape is checked; the text is kept.
pub const Date = struct : Value { s: str, kind: i64 };

pub const OFFSET_DATE_TIME = 0;
pub const LOCAL_DATE_TIME = 1;
pub const LOCAL_DATE = 2;
pub const LOCAL_TIME = 3;

/// `literal` is true for an array written `[ .. ]`, which is closed, and false
/// for the one `[[header]]` builds, which is not.
pub const Arr = struct : Value { items: list.List[Value], literal: bool };

/// A table, and how it came to exist.
///
/// The three flags are not decoration -- they *are* TOML's redefinition rules:
/// a table may be created implicitly by a header below it and defined later,
/// but one already defined by its own header may not be defined again, one
/// written `{ .. }` may not be added to at all, and one a dotted key brought
/// into being may not afterwards be named by a header.
pub const Table = struct : Value {
    keys: list.List[str],
    vals: list.List[Value],
    /// Written `{ .. }`. Closed the moment its brace closes.
    braced: bool,
    /// Named by its own `[header]`.
    explicit: bool,
    /// Brought into being as an intermediate of a dotted key.
    dotted: bool,
};

pub fn table() Table {
    return Table{
        .keys = list.new(),
        .vals = list.new(),
        .braced = false,
        .explicit = false,
        .dotted = false,
    };
}

// ---------------------------------------------------------------------------
// Making values
// ---------------------------------------------------------------------------
//
// Every constructor answers with a `Value` rather than with its own type,
// because a subtype does not coerce into a supertype at a call argument -- only
// at an annotated binding. One `const v: Value = ..` per shape, here, is what
// keeps that spelling out of every call site.

pub fn of_str(s: str) Value { const v: Value = Str{ .s = s }; return v; }
pub fn of_int(n: i64) Value { const v: Value = Int{ .n = n }; return v; }
pub fn of_float(f: f64) Value { const v: Value = Float{ .f = f }; return v; }
pub fn of_bool(b: bool) Value { const v: Value = Bool{ .b = b }; return v; }
pub fn of_date(s: str, kind: i64) Value { const v: Value = Date{ .s = s, .kind = kind }; return v; }
pub fn of_table(t: Table) Value { const v: Value = t; return v; }

pub fn of_array(items: list.List[Value], literal: bool) Value {
    const v: Value = Arr{ .items = items, .literal = literal };
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

pub fn as_int(v: Value) !{BadFormat}i64 { return error.BadFormat; }
pub fn as_int(v: Int) !{BadFormat}i64 { return v.n; }

pub fn as_float(v: Value) !{BadFormat}f64 { return error.BadFormat; }
pub fn as_float(v: Float) !{BadFormat}f64 { return v.f; }

pub fn as_bool(v: Value) !{BadFormat}bool { return error.BadFormat; }
pub fn as_bool(v: Bool) !{BadFormat}bool { return v.b; }

pub fn as_date(v: Value) !{BadFormat}str { return error.BadFormat; }
pub fn as_date(v: Date) !{BadFormat}str { return v.s; }

pub fn as_array(v: Value) !{BadFormat}list.List[Value] { return error.BadFormat; }
pub fn as_array(v: Arr) !{BadFormat}list.List[Value] { return v.items; }

pub fn as_table(v: Value) !{BadFormat}Table { return error.BadFormat; }
pub fn as_table(v: Table) !{BadFormat}Table { return v; }

/// What shape a value is, for a caller that would rather ask than catch.
pub const KIND_NONE = 0;
pub const KIND_STR = 1;
pub const KIND_INT = 2;
pub const KIND_FLOAT = 3;
pub const KIND_BOOL = 4;
pub const KIND_DATE = 5;
pub const KIND_ARRAY = 6;
pub const KIND_TABLE = 7;

pub fn kind(v: Value) i64 { return KIND_NONE; }
pub fn kind(v: Str) i64 { return KIND_STR; }
pub fn kind(v: Int) i64 { return KIND_INT; }
pub fn kind(v: Float) i64 { return KIND_FLOAT; }
pub fn kind(v: Bool) i64 { return KIND_BOOL; }
pub fn kind(v: Date) i64 { return KIND_DATE; }
pub fn kind(v: Arr) i64 { return KIND_ARRAY; }
pub fn kind(v: Table) i64 { return KIND_TABLE; }

// ---------------------------------------------------------------------------
// Tables
// ---------------------------------------------------------------------------
//
// Parallel key and value lists with a linear scan, because `std` has no hash
// map and a manifest has tens of keys rather than thousands. Said out loud
// rather than left to be discovered: if something ever puts a large table
// through this, the fix is a map and not a cleverer scan.

pub fn get(t: Table, key: str) ?Value {
    const n = list.len(t.keys);
    var i = 0;
    while (i < n) : (i += 1) {
        if (text.eq(list.get(t.keys, i), key)) { return list.get(t.vals, i); }
    }
    return null;
}

pub fn has(t: Table, key: str) bool {
    return index_of(t, key) >= 0;
}

/// The keys this table holds, in the order they were written.
pub fn keys(t: Table) []str { return list.to_array(t.keys); }

pub fn len(t: Table) i64 { return list.len(t.keys); }

/// Add or replace one entry.
pub fn set(t: Table, key: str, v: Value) void {
    const at = index_of(t, key);
    if (at >= 0) { list.set(t.vals, at, v); return; }
    list.push(t.keys, key);
    list.push(t.vals, v);
    return;
}

fn index_of(t: Table, key: str) i64 {
    const n = list.len(t.keys);
    var i = 0;
    while (i < n) : (i += 1) {
        if (text.eq(list.get(t.keys, i), key)) { return i; }
    }
    return -1;
}

/// The value at a dotted path, or null.
///
/// `lookup(doc.root, "package.name")` is what reading a manifest looks like,
/// and it walks tables only -- an array in the middle is not a path.
pub fn lookup(t: Table, path: str) ?Value {
    var here = t;
    const parts = text.split(path, ".");
    const n = array.len(parts);
    var i = 0;
    while (i < n) : (i += 1) {
        const found = get(here, parts[i]) orelse return null;
        if (i == n - 1) { return found; }
        here = as_table(found) catch return null;
    }
    return null;
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// What `parse` answers with.
///
/// `ok` says which half to read. `line` is where the reader was when it gave
/// up, counting from one, so a caller can point at it.
pub const Doc = struct { root: Table, ok: bool, message: str, line: i64 };

const P = struct {
    b: []u8,
    at: i64,
    n: i64,
    line: i64,
    ok: bool,
    message: str,
    fail_line: i64,
};

/// Read a whole TOML document.
pub fn parse(src: str) Doc {
    const b = bytes.of(src);
    const p = P{
        .b = b,
        .at = 0,
        .n = array.len(b),
        .line = 1,
        .ok = true,
        .message = "",
        .fail_line = 0,
    };
    const root = table();
    root.explicit = true;
    var current = root;

    while (p.ok) {
        skip_blank(p);
        if (at_end(p)) { break; }
        if (peek(p) == LBRACKET) {
            current = header(p, root);
        } else {
            key_value(p, current);
        }
        if (!p.ok) { break; }
        end_of_line(p);
    }
    return Doc{ .root = root, .ok = p.ok, .message = p.message, .line = p.fail_line };
}

// ---- bytes -----------------------------------------------------------------

const TAB = 9;
const LF = 10;
const CR = 13;
const SPACE = 32;
const QUOTE = 34;
const HASH = 35;
const APOS = 39;
const PLUS = 43;
const COMMA = 44;
const MINUS = 45;
const DOT = 46;
const COLON = 58;
const EQUALS = 61;
const LBRACKET = 91;
const BACKSLASH = 92;
const RBRACKET = 93;
const UNDERSCORE = 95;
const LBRACE = 123;
const RBRACE = 125;

fn at_end(p: P) bool { return p.at >= p.n; }

/// The byte under the cursor, or -1 at the end.
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
    }
    return;
}

fn is_digit(c: i64) bool { return c >= 48 and c <= 57; }
fn is_hex(c: i64) bool {
    return is_digit(c) or (c >= 97 and c <= 102) or (c >= 65 and c <= 70);
}
fn is_bare(c: i64) bool {
    return is_digit(c) or (c >= 97 and c <= 122) or (c >= 65 and c <= 90)
        or c == UNDERSCORE or c == MINUS;
}

/// A control character that may not appear raw in a string or a comment.
///
/// Everything below space except tab, and DEL. Rejecting these is what stops a
/// manifest carrying an escape sequence into somebody's terminal.
fn is_control(c: i64) bool {
    return (c >= 0 and c < 32 and c != TAB) or c == 127;
}

fn skip_spaces(p: P) void {
    while (peek(p) == SPACE or peek(p) == TAB) { p.at += 1; }
    return;
}

/// A `#` and the rest of its line.
fn skip_comment(p: P) void {
    if (peek(p) != HASH) { return; }
    p.at += 1;
    while (!at_end(p)) {
        const c = peek(p);
        if (c == LF) { return; }
        if (c == CR and peek_at(p, 1) == LF) { return; }
        if (is_control(c)) { fail(p, "a comment cannot hold a control character"); return; }
        p.at += 1;
    }
    return;
}

/// A newline, if one is there. `\r` is only a newline when `\n` follows.
fn take_newline(p: P) bool {
    if (peek(p) == LF) { bump(p); return true; }
    if (peek(p) == CR and peek_at(p, 1) == LF) { p.at += 1; bump(p); return true; }
    return false;
}

/// Whitespace, comments and newlines: what may sit between two statements.
fn skip_blank(p: P) void {
    while (p.ok) {
        skip_spaces(p);
        skip_comment(p);
        if (!p.ok) { return; }
        if (!take_newline(p)) { return; }
    }
    return;
}

/// What must follow a statement: optional spaces, an optional comment, and
/// then a newline or the end of the file.
fn end_of_line(p: P) void {
    skip_spaces(p);
    skip_comment(p);
    if (!p.ok) { return; }
    if (at_end(p)) { return; }
    if (!take_newline(p)) { fail(p, "expected a newline after this"); }
    return;
}

// ---- keys ------------------------------------------------------------------

/// A dotted key path: one or more simple keys separated by `.`.
fn key_path(p: P) list.List[str] {
    var out: list.List[str] = list.new();
    var going = true;
    while (going and p.ok) {
        list.push(out, simple_key(p));
        skip_spaces(p);
        if (peek(p) == DOT) { p.at += 1; skip_spaces(p); } else { going = false; }
    }
    return out;
}

fn simple_key(p: P) str {
    const c = peek(p);
    if (c == QUOTE) { return basic_string(p); }
    if (c == APOS) { return literal_string(p); }
    const start = p.at;
    while (is_bare(peek(p))) { p.at += 1; }
    if (p.at == start) { fail(p, "expected a key"); return ""; }
    return bytes.slice_str(p.b, start, p.at);
}

// ---- statements ------------------------------------------------------------

/// `key = value`, written into `current`.
fn key_value(p: P, current: Table) void {
    const path = key_path(p);
    if (!p.ok) { return; }
    skip_spaces(p);
    if (peek(p) != EQUALS) { fail(p, "expected `=` after a key"); return; }
    p.at += 1;
    skip_spaces(p);
    const v = value(p);
    if (!p.ok) { return; }
    assign(p, current, path, v);
    return;
}

/// Write `v` at `path` under `into`, creating dotted intermediates.
fn assign(p: P, into: Table, path: list.List[str], v: Value) void {
    var here = into;
    const depth = list.len(path);
    var i = 0;
    while (i < depth - 1) : (i += 1) {
        const name = list.get(path, i);
        const found = get(here, name);
        if (found) |existing| {
            const next = as_table(existing) catch {
                fail(p, text.concat(text.concat("`", name), "` is not a table"));
                return;
            };
            if (next.braced) {
                fail(p, text.concat(text.concat("`", name), "` was written as an inline table, which is closed"));
                return;
            }
            // A table a `[header]` defined cannot be reached back into with a
            // dotted key from somewhere else; one a dotted key made can.
            if (next.explicit and !next.dotted) {
                fail(p, text.concat(text.concat("`", name), "` is already defined by a header"));
                return;
            }
            here = next;
        } else {
            const made = table();
            made.dotted = true;
            set(here, name, of_table(made));
            here = made;
        }
    }
    const last = list.get(path, depth - 1);
    if (has(here, last)) {
        fail(p, text.concat(text.concat("`", last), "` is defined more than once"));
        return;
    }
    set(here, last, v);
    return;
}

/// `[a.b]` or `[[a.b]]`, answering with the table that follows it.
fn header(p: P, root: Table) Table {
    p.at += 1;
    const is_array = peek(p) == LBRACKET;
    if (is_array) { p.at += 1; }
    skip_spaces(p);
    const path = key_path(p);
    if (!p.ok) { return root; }
    skip_spaces(p);
    if (peek(p) != RBRACKET) { fail(p, "expected `]` to close a table header"); return root; }
    p.at += 1;
    if (is_array) {
        if (peek(p) != RBRACKET) { fail(p, "expected `]]` to close an array-of-tables header"); return root; }
        p.at += 1;
    }

    var here = root;
    const depth = list.len(path);
    var i = 0;
    while (i < depth - 1) : (i += 1) {
        here = descend(p, here, list.get(path, i));
        if (!p.ok) { return root; }
    }
    const name = list.get(path, depth - 1);

    if (is_array) { return append_table(p, here, name); }
    return define_table(p, here, name);
}

/// One step of a header's path, creating the table if it is not there.
///
/// An array of tables in the middle means the *last* table it holds, which is
/// what makes `[[a]] ... [a.b]` name a `b` inside the second `a`.
fn descend(p: P, here: Table, name: str) Table {
    const found = get(here, name);
    if (found) |existing| {
        const arr = as_array(existing) catch {
            const t = as_table(existing) catch {
                fail(p, text.concat(text.concat("`", name), "` is not a table"));
                return here;
            };
            if (t.braced) {
                fail(p, text.concat(text.concat("`", name), "` was written as an inline table, which is closed"));
                return here;
            }
            return t;
        };
        const n = list.len(arr);
        if (n == 0) {
            fail(p, text.concat(text.concat("`", name), "` is an array with no table in it"));
            return here;
        }
        return as_table(list.get(arr, n - 1)) catch {
            fail(p, text.concat(text.concat("`", name), "` is an array of values, not of tables"));
            return here;
        };
    }
    const made = table();
    set(here, name, of_table(made));
    return made;
}

fn define_table(p: P, here: Table, name: str) Table {
    const found = get(here, name);
    if (found) |existing| {
        const t = as_table(existing) catch {
            fail(p, text.concat(text.concat("`", name), "` is not a table"));
            return here;
        };
        if (t.explicit or t.braced or t.dotted) {
            fail(p, text.concat(text.concat("`", name), "` is defined more than once"));
            return here;
        }
        t.explicit = true;
        return t;
    }
    const made = table();
    made.explicit = true;
    set(here, name, of_table(made));
    return made;
}

fn append_table(p: P, here: Table, name: str) Table {
    const made = table();
    made.explicit = true;
    const found = get(here, name);
    if (found) |existing| {
        const arr = as_array(existing) catch {
            fail(p, text.concat(text.concat("`", name), "` is not an array of tables"));
            return here;
        };
        // The one written `[ .. ]` is closed; only the one a header built grows.
        if (is_literal_array(existing)) {
            fail(p, text.concat(text.concat("`", name), "` is a static array and cannot be extended"));
            return here;
        }
        list.push(arr, of_table(made));
        return made;
    }
    var items: list.List[Value] = list.new();
    list.push(items, of_table(made));
    set(here, name, of_array(items, false));
    return made;
}

/// The one question `as_array` cannot answer: whether it was written `[ .. ]`
/// or built by a `[[header]]`. Only the second may grow.
fn is_literal_array(v: Value) bool { return false; }
fn is_literal_array(v: Arr) bool { return v.literal; }

// ---- values ----------------------------------------------------------------

fn value(p: P) Value {
    const c = peek(p);
    if (c == QUOTE) { return of_str(basic_string(p)); }
    if (c == APOS) { return of_str(literal_string(p)); }
    if (c == LBRACKET) { return array_value(p); }
    if (c == LBRACE) { return inline_table(p); }
    if (c < 0) { fail(p, "expected a value"); return Value{}; }
    return scalar(p);
}

/// An array, which may span lines and may hold anything.
fn array_value(p: P) Value {
    p.at += 1;
    var items: list.List[Value] = list.new();
    var going = true;
    while (going and p.ok) {
        skip_blank(p);
        if (!p.ok) { return Value{}; }
        if (peek(p) == RBRACKET) { p.at += 1; going = false; continue; }
        if (at_end(p)) { fail(p, "expected `]` to close an array"); return Value{}; }
        list.push(items, value(p));
        if (!p.ok) { return Value{}; }
        skip_blank(p);
        if (peek(p) == COMMA) { p.at += 1; continue; }
        skip_blank(p);
        if (peek(p) == RBRACKET) { p.at += 1; going = false; continue; }
        fail(p, "expected `,` or `]` in an array");
    }
    return of_array(items, true);
}

/// `{ k = v, .. }`, which may not span lines and is closed once written.
fn inline_table(p: P) Value {
    p.at += 1;
    const t = table();
    t.braced = true;
    skip_spaces(p);
    if (peek(p) == RBRACE) { p.at += 1; return of_table(t); }
    var going = true;
    while (going and p.ok) {
        skip_spaces(p);
        key_value_inline(p, t);
        if (!p.ok) { return Value{}; }
        skip_spaces(p);
        if (peek(p) == COMMA) {
            p.at += 1;
            skip_spaces(p);
            // TOML 1.0 has no trailing comma in an inline table, and saying so
            // is friendlier than the "expected a key" it would otherwise be.
            if (peek(p) == RBRACE) { fail(p, "an inline table cannot end with a comma"); return Value{}; }
            continue;
        }
        if (peek(p) == RBRACE) { p.at += 1; going = false; continue; }
        fail(p, "expected `,` or `}` in an inline table");
    }
    return of_table(t);
}

fn key_value_inline(p: P, t: Table) void {
    const path = key_path(p);
    if (!p.ok) { return; }
    skip_spaces(p);
    if (peek(p) != EQUALS) { fail(p, "expected `=` after a key"); return; }
    p.at += 1;
    skip_spaces(p);
    const v = value(p);
    if (!p.ok) { return; }
    assign(p, t, path, v);
    return;
}

// ---- scalars ---------------------------------------------------------------

/// Everything that is not a string, an array or a table: `true`, `false`, a
/// number, or one of the four date and time forms.
///
/// Lexed whole and then classified, rather than decided one byte at a time,
/// because the alternative is a look-ahead that has to tell `1979-05-27` from
/// `1979` and `-27` before it has read either. The one subtlety is that a local
/// date-time may hold a space -- `1979-05-27 07:32:00` -- so the lexer keeps
/// going across one when a date is behind it and a time is in front.
fn scalar(p: P) Value {
    const start = p.at;
    var going = true;
    while (going) {
        const c = peek(p);
        if (c < 0 or c == COMMA or c == RBRACKET or c == RBRACE or c == HASH
            or c == LF or c == CR) {
            going = false;
        } else {
            if (c == SPACE or c == TAB) {
                if (c == SPACE and looks_like_date(p.b, start, p.at) and time_follows(p, 1)) {
                    p.at += 1;
                } else {
                    going = false;
                }
            } else {
                p.at += 1;
            }
        }
    }
    const raw = bytes.slice_str(p.b, start, p.at);
    if (text.len(raw) == 0) { fail(p, "expected a value"); return Value{}; }
    if (text.eq(raw, "true")) { return of_bool(true); }
    if (text.eq(raw, "false")) { return of_bool(false); }
    if (looks_like_date(p.b, start, p.at) or looks_like_time(p.b, start, p.at)) {
        return date_value(p, raw);
    }
    return number(p, raw);
}

/// `dddd-` at the start of the lexeme.
fn looks_like_date(b: []u8, from: i64, to: i64) bool {
    if (to - from < 5) { return false; }
    var i = 0;
    while (i < 4) : (i += 1) {
        if (!is_digit(i64(b[from + i]))) { return false; }
    }
    return i64(b[from + 4]) == MINUS;
}

/// `dd:` at the start of the lexeme.
fn looks_like_time(b: []u8, from: i64, to: i64) bool {
    if (to - from < 3) { return false; }
    return is_digit(i64(b[from])) and is_digit(i64(b[from + 1]))
        and i64(b[from + 2]) == COLON;
}

/// `dd:` `k` bytes further on, which is what makes a space part of a value.
fn time_follows(p: P, k: i64) bool {
    return is_digit(peek_at(p, k)) and is_digit(peek_at(p, k + 1))
        and peek_at(p, k + 2) == COLON;
}

// ---- strings ---------------------------------------------------------------

fn basic_string(p: P) str {
    if (peek_at(p, 1) == QUOTE and peek_at(p, 2) == QUOTE) { return multiline_basic(p); }
    p.at += 1;
    const b = bytes.buf(32);
    var going = true;
    while (going and p.ok) {
        const c = peek(p);
        if (c < 0 or c == LF or (c == CR and peek_at(p, 1) == LF)) {
            fail(p, "a basic string is not closed before the end of its line");
            return "";
        }
        if (c == QUOTE) { p.at += 1; going = false; continue; }
        if (c == BACKSLASH) { p.at += 1; escape(p, b, false); continue; }
        if (is_control(c)) { fail(p, "a string cannot hold a raw control character"); return ""; }
        bytes.put_u8(b, c);
        p.at += 1;
    }
    return bytes.to_str(bytes.taken(b));
}

/// `"""..."""`.
///
/// A newline straight after the opening delimiter is not part of the value; a
/// `\` at the end of a line swallows the newline and the indentation after it;
/// and one or two quotes may stand inside without being escaped, which is why
/// the close is found by counting a run rather than by matching three.
///
/// CRLF becomes LF. TOML lets a reader normalise, and a string whose contents
/// depend on which machine checked the file out is worse than one that does
/// not.
fn multiline_basic(p: P) str {
    p.at += 3;
    take_newline(p);
    const b = bytes.buf(64);
    var going = true;
    while (going and p.ok) {
        const c = peek(p);
        if (c < 0) { fail(p, "a multi-line string is not closed"); return ""; }
        if (c == QUOTE) {
            const run = quote_run(p, QUOTE);
            if (run < 3) {
                var i = 0;
                while (i < run) : (i += 1) { bytes.put_u8(b, QUOTE); }
                p.at += run;
                continue;
            }
            if (run > 5) { fail(p, "too many quotation marks to end a multi-line string"); return ""; }
            var i = 0;
            while (i < run - 3) : (i += 1) { bytes.put_u8(b, QUOTE); }
            p.at += run;
            going = false;
            continue;
        }
        if (c == BACKSLASH) { p.at += 1; escape(p, b, true); continue; }
        if (c == CR and peek_at(p, 1) == LF) { p.at += 1; continue; }
        if (is_control(c) and c != LF) { fail(p, "a string cannot hold a raw control character"); return ""; }
        bytes.put_u8(b, c);
        bump(p);
    }
    return bytes.to_str(bytes.taken(b));
}

fn literal_string(p: P) str {
    if (peek_at(p, 1) == APOS and peek_at(p, 2) == APOS) { return multiline_literal(p); }
    p.at += 1;
    const start = p.at;
    var going = true;
    while (going and p.ok) {
        const c = peek(p);
        if (c < 0 or c == LF or (c == CR and peek_at(p, 1) == LF)) {
            fail(p, "a literal string is not closed before the end of its line");
            return "";
        }
        if (c == APOS) { going = false; continue; }
        if (is_control(c)) { fail(p, "a string cannot hold a raw control character"); return ""; }
        p.at += 1;
    }
    const out = bytes.slice_str(p.b, start, p.at);
    p.at += 1;
    return out;
}

fn multiline_literal(p: P) str {
    p.at += 3;
    take_newline(p);
    const b = bytes.buf(64);
    var going = true;
    while (going and p.ok) {
        const c = peek(p);
        if (c < 0) { fail(p, "a multi-line literal string is not closed"); return ""; }
        if (c == APOS) {
            const run = quote_run(p, APOS);
            if (run < 3) {
                var i = 0;
                while (i < run) : (i += 1) { bytes.put_u8(b, APOS); }
                p.at += run;
                continue;
            }
            if (run > 5) { fail(p, "too many apostrophes to end a multi-line string"); return ""; }
            var i = 0;
            while (i < run - 3) : (i += 1) { bytes.put_u8(b, APOS); }
            p.at += run;
            going = false;
            continue;
        }
        if (c == CR and peek_at(p, 1) == LF) { p.at += 1; continue; }
        if (is_control(c) and c != LF) { fail(p, "a string cannot hold a raw control character"); return ""; }
        bytes.put_u8(b, c);
        bump(p);
    }
    return bytes.to_str(bytes.taken(b));
}

/// How many of `q` are under the cursor.
fn quote_run(p: P, q: i64) i64 {
    var n = 0;
    while (peek_at(p, n) == q) : (n += 1) { }
    return n;
}

/// What follows a backslash. The backslash itself is already consumed.
fn escape(p: P, b: bytes.Buf, multiline: bool) void {
    const c = peek(p);
    // A backslash at the end of a line swallows the newline and every space in
    // front of the next line's first character, which is what lets a long
    // string be indented in the file and not in the value.
    if (multiline and (c == SPACE or c == TAB or c == LF or (c == CR and peek_at(p, 1) == LF))) {
        var seen_newline = false;
        var going = true;
        while (going) {
            const w = peek(p);
            if (w == SPACE or w == TAB) { p.at += 1; continue; }
            if (take_newline(p)) { seen_newline = true; continue; }
            going = false;
        }
        if (!seen_newline) { fail(p, "a backslash must be followed by an escape or by the end of the line"); }
        return;
    }
    p.at += 1;
    if (c == 110) { bytes.put_u8(b, LF); return; }
    if (c == 116) { bytes.put_u8(b, TAB); return; }
    if (c == 114) { bytes.put_u8(b, CR); return; }
    if (c == 98) { bytes.put_u8(b, 8); return; }
    if (c == 102) { bytes.put_u8(b, 12); return; }
    if (c == QUOTE) { bytes.put_u8(b, QUOTE); return; }
    if (c == BACKSLASH) { bytes.put_u8(b, BACKSLASH); return; }
    if (c == 117) { unicode(p, b, 4); return; }
    if (c == 85) { unicode(p, b, 8); return; }
    fail(p, "unknown escape sequence: the ones TOML has are \\b \\t \\n \\f \\r \\\" \\\\ \\uXXXX \\UXXXXXXXX");
    return;
}

/// `\uXXXX` or `\UXXXXXXXX`, encoded as UTF-8.
fn unicode(p: P, b: bytes.Buf, width: i64) void {
    var cp = 0;
    var i = 0;
    while (i < width) : (i += 1) {
        const c = peek(p);
        if (!is_hex(c)) { fail(p, "an escaped code point needs its full complement of hex digits"); return; }
        cp = cp * 16 + hex_value(c);
        p.at += 1;
    }
    // The surrogate range is not a character; a file that names one is a file
    // that came from a UTF-16 encoder that did not finish the job.
    if (cp > 1114111 or (cp >= 55296 and cp <= 57343)) {
        fail(p, "an escaped code point must be a scalar value");
        return;
    }
    bytes.put_utf8(b, cp);
    return;
}

fn hex_value(c: i64) i64 {
    if (c >= 48 and c <= 57) { return c - 48; }
    if (c >= 97 and c <= 102) { return c - 87; }
    return c - 55;
}

// ---- numbers ---------------------------------------------------------------

/// An integer or a float, from the lexeme `raw`.
///
/// The value itself is handed to `str.parse_int` or `str.parse_float`, which
/// are correctly rounded and range-checked; what happens here is TOML's
/// *grammar*, which is stricter than either. `1_0` is ten and `1__0` is not;
/// `0.1` is a float and `.1` is not a number at all; `007` is not seven.
fn number(p: P, raw: str) Value {
    var s = raw;
    var negative = false;
    var signed = false;
    const first = text.byte_at(s, 0);
    if (first == MINUS or first == PLUS) {
        negative = first == MINUS;
        signed = true;
        s = text.substr(s, 1, text.len(s));
    }
    if (text.len(s) == 0) { fail(p, "a sign with no number after it"); return Value{}; }

    if (text.eq(s, "inf")) { return of_float(infinity(negative)); }
    if (text.eq(s, "nan")) { return of_float(not_a_number()); }

    // A radix prefix takes no sign: TOML has no `-0xff`.
    if (text.len(s) > 2 and text.byte_at(s, 0) == 48) {
        const marker = text.byte_at(s, 1);
        if (marker == 120 or marker == 111 or marker == 98) {
            if (signed) { fail(p, "a hexadecimal, octal or binary number takes no sign"); return Value{}; }
            var radix = 16;
            if (marker == 111) { radix = 8; }
            if (marker == 98) { radix = 2; }
            return radix_number(p, text.substr(s, 2, text.len(s)), radix);
        }
    }

    const digits = clean_digits(p, s, 10);
    if (!p.ok) { return Value{}; }
    if (is_float_text(s)) {
        if (!float_shape_ok(digits)) { fail(p, "a float needs digits on both sides of its point and after its exponent"); return Value{}; }
        const f = text.parse_float(if (negative) text.concat("-", digits) else digits) catch {
            fail(p, "this is not a number this machine can hold");
            return Value{};
        };
        return of_float(f);
    }
    if (!integer_shape_ok(digits)) { fail(p, "an integer may not have a leading zero"); return Value{}; }
    const n = text.parse_int(if (negative) text.concat("-", digits) else digits) catch {
        fail(p, "this integer does not fit in 64 bits");
        return Value{};
    };
    return of_int(n);
}

/// Underscores removed, having checked that each one sits between two digits.
fn clean_digits(p: P, s: str, radix: i64) str {
    const n = text.len(s);
    const b = bytes.buf(n);
    var i = 0;
    while (i < n) : (i += 1) {
        const c = text.byte_at(s, i);
        if (c == UNDERSCORE) {
            if (i == 0 or i == n - 1
                or !is_radix_digit(text.byte_at(s, i - 1), radix)
                or !is_radix_digit(text.byte_at(s, i + 1), radix)) {
                fail(p, "an underscore in a number must sit between two digits");
                return "";
            }
            continue;
        }
        bytes.put_u8(b, c);
    }
    return bytes.to_str(bytes.taken(b));
}

fn is_radix_digit(c: i64, radix: i64) bool {
    if (radix == 16) { return is_hex(c); }
    if (radix == 8) { return c >= 48 and c <= 55; }
    if (radix == 2) { return c == 48 or c == 49; }
    return is_digit(c);
}

/// A number with a `.` or an `e` in it is a float. The `e` test skips the
/// first character so that `nan` and `inf` -- already handled -- and a leading
/// `e` cannot be read as an exponent.
fn is_float_text(s: str) bool {
    const n = text.len(s);
    var i = 0;
    while (i < n) : (i += 1) {
        const c = text.byte_at(s, i);
        if (c == DOT) { return true; }
        if ((c == 101 or c == 69) and i > 0) { return true; }
    }
    return false;
}

fn integer_shape_ok(s: str) bool {
    const n = text.len(s);
    if (n == 0) { return false; }
    var i = 0;
    while (i < n) : (i += 1) {
        if (!is_digit(text.byte_at(s, i))) { return false; }
    }
    return n == 1 or text.byte_at(s, 0) != 48;
}

/// TOML's float grammar, which is narrower than what a C library would take:
/// digits before the point, digits after it, and digits after the exponent.
fn float_shape_ok(s: str) bool {
    const n = text.len(s);
    var i = 0;
    // The whole part.
    const int_start = i;
    while (i < n and is_digit(text.byte_at(s, i))) : (i += 1) { }
    if (i == int_start) { return false; }
    if (i - int_start > 1 and text.byte_at(s, int_start) == 48) { return false; }
    if (i < n and text.byte_at(s, i) == DOT) {
        i += 1;
        const frac_start = i;
        while (i < n and is_digit(text.byte_at(s, i))) : (i += 1) { }
        if (i == frac_start) { return false; }
    }
    if (i < n and (text.byte_at(s, i) == 101 or text.byte_at(s, i) == 69)) {
        i += 1;
        if (i < n and (text.byte_at(s, i) == PLUS or text.byte_at(s, i) == MINUS)) { i += 1; }
        const exp_start = i;
        while (i < n and is_digit(text.byte_at(s, i))) : (i += 1) { }
        if (i == exp_start) { return false; }
    }
    return i == n;
}

/// `0x`, `0o` or `0b`, accumulated by hand.
///
/// `str.parse_int` is decimal, and the overflow check is written out here for
/// the reason it is written out there: `*` wraps in this language, so a number
/// too big to hold would quietly become a different one.
fn radix_number(p: P, body: str, radix: i64) Value {
    const digits = clean_digits(p, body, radix);
    if (!p.ok) { return Value{}; }
    const n = text.len(digits);
    if (n == 0) { fail(p, "a radix prefix with no digits after it"); return Value{}; }
    var v = 0;
    var i = 0;
    while (i < n) : (i += 1) {
        const c = text.byte_at(digits, i);
        if (!is_radix_digit(c, radix)) { fail(p, "a digit that is not valid in this base"); return Value{}; }
        const d = hex_value(c);
        if (v > (9223372036854775807 - d) / radix) {
            fail(p, "this integer does not fit in 64 bits");
            return Value{};
        }
        v = v * radix + d;
    }
    return of_int(v);
}

/// Made through the parser rather than written, because W# has no literal for
/// either and `1.0 / 0.0` is a way of saying it that depends on what the
/// hardware does with a division.
fn infinity(negative: bool) f64 {
    if (negative) { return text.parse_float("-inf") catch 0.0; }
    return text.parse_float("inf") catch 0.0;
}

fn not_a_number() f64 { return text.parse_float("nan") catch 0.0; }

// ---- dates and times -------------------------------------------------------

/// One of TOML's four date and time forms, validated and kept as written.
fn date_value(p: P, raw: str) Value {
    const n = text.len(raw);
    if (looks_like_time(bytes.of(raw), 0, n)) {
        if (!time_ok(raw, 0, n)) { fail(p, "this is not a valid time"); return Value{}; }
        return of_date(raw, LOCAL_TIME);
    }
    if (!date_ok(raw, 0)) { fail(p, "this is not a valid date"); return Value{}; }
    if (n == 10) { return of_date(raw, LOCAL_DATE); }
    const sep = text.byte_at(raw, 10);
    if (sep != 84 and sep != 116 and sep != SPACE) {
        fail(p, "a date and a time are joined by `T` or by a space");
        return Value{};
    }
    // Where the time ends: an offset begins at `Z`, `z`, `+`, or the `-` that
    // is not part of a fraction.
    var end = n;
    var i = 11;
    while (i < n) : (i += 1) {
        const c = text.byte_at(raw, i);
        if (c == 90 or c == 122 or c == PLUS or c == MINUS) { end = i; break; }
    }
    if (!time_ok(raw, 11, end)) { fail(p, "this is not a valid time"); return Value{}; }
    if (end == n) { return of_date(raw, LOCAL_DATE_TIME); }
    if (!offset_ok(raw, end, n)) { fail(p, "this is not a valid UTC offset"); return Value{}; }
    return of_date(raw, OFFSET_DATE_TIME);
}

/// `YYYY-MM-DD` at `from`, with the day checked against the month and the year.
fn date_ok(s: str, from: i64) bool {
    if (text.len(s) < from + 10) { return false; }
    const year = digits_value(s, from, 4);
    if (year < 0) { return false; }
    if (text.byte_at(s, from + 4) != MINUS) { return false; }
    const month = digits_value(s, from + 5, 2);
    if (month < 1 or month > 12) { return false; }
    if (text.byte_at(s, from + 7) != MINUS) { return false; }
    const day = digits_value(s, from + 8, 2);
    return day >= 1 and day <= days_in(year, month);
}

fn days_in(year: i64, month: i64) i64 {
    if (month == 2) {
        if (year % 4 == 0 and (year % 100 != 0 or year % 400 == 0)) { return 29; }
        return 28;
    }
    if (month == 4 or month == 6 or month == 9 or month == 11) { return 30; }
    return 31;
}

/// `HH:MM:SS` with an optional fraction, over `from .. to`.
///
/// A second of 60 is allowed, because a leap second is a real time that a real
/// clock has produced.
fn time_ok(s: str, from: i64, to: i64) bool {
    if (to - from < 8) { return false; }
    const hour = digits_value(s, from, 2);
    if (hour < 0 or hour > 23) { return false; }
    if (text.byte_at(s, from + 2) != COLON) { return false; }
    const minute = digits_value(s, from + 3, 2);
    if (minute < 0 or minute > 59) { return false; }
    if (text.byte_at(s, from + 5) != COLON) { return false; }
    const second = digits_value(s, from + 6, 2);
    if (second < 0 or second > 60) { return false; }
    if (to - from == 8) { return true; }
    if (text.byte_at(s, from + 8) != DOT) { return false; }
    var i = from + 9;
    if (i >= to) { return false; }
    while (i < to) : (i += 1) {
        if (!is_digit(text.byte_at(s, i))) { return false; }
    }
    return true;
}

/// `Z`, `z`, or `+HH:MM` / `-HH:MM`.
fn offset_ok(s: str, from: i64, to: i64) bool {
    const c = text.byte_at(s, from);
    if (c == 90 or c == 122) { return to - from == 1; }
    if (c != PLUS and c != MINUS) { return false; }
    if (to - from != 6) { return false; }
    const hour = digits_value(s, from + 1, 2);
    if (hour < 0 or hour > 23) { return false; }
    if (text.byte_at(s, from + 3) != COLON) { return false; }
    const minute = digits_value(s, from + 4, 2);
    return minute >= 0 and minute <= 59;
}

/// `width` digits at `at`, as a number, or -1 if they are not all digits.
fn digits_value(s: str, at: i64, width: i64) i64 {
    if (text.len(s) < at + width) { return -1; }
    var v = 0;
    var i = 0;
    while (i < width) : (i += 1) {
        const c = text.byte_at(s, at + i);
        if (!is_digit(c)) { return -1; }
        v = v * 10 + (c - 48);
    }
    return v;
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------
//
// `std/der` has no writer, on the grounds that nothing needed one. Something
// needs one here -- a lockfile is written as well as read -- and the reason to
// write it properly rather than to concatenate strings at the call site is that
// a lockfile which cannot be read back by the reader beside it is a bug nobody
// finds until the day it matters. The round trip is the test.

/// Render a table as TOML.
///
/// Sub-tables become `[headers]` and arrays of tables become `[[headers]]`,
/// which means the order is not the order the table holds: TOML puts a key that
/// belongs to a table *before* the first header below it, or it belongs to the
/// wrong table. Scalars first, then tables, then arrays of tables, at every
/// level.
pub fn write(t: Table) str {
    const b = bytes.buf(256);
    write_table(b, t, "");
    return bytes.to_str(bytes.taken(b));
}

fn write_table(b: bytes.Buf, t: Table, prefix: str) void {
    const n = list.len(t.keys);
    var i = 0;
    while (i < n) : (i += 1) {
        const v = list.get(t.vals, i);
        if (is_header_table(v) or is_table_array(v)) { continue; }
        bytes.put_str(b, write_key(list.get(t.keys, i)));
        bytes.put_str(b, " = ");
        write_value(b, v);
        bytes.put_u8(b, LF);
    }
    i = 0;
    while (i < n) : (i += 1) {
        const key = list.get(t.keys, i);
        const v = list.get(t.vals, i);
        const path = join_key(prefix, key);
        if (is_header_table(v)) {
            gap(b);
            bytes.put_str(b, "[");
            bytes.put_str(b, path);
            bytes.put_str(b, "]\n");
            write_table(b, as_table(v) catch table(), path);
            continue;
        }
        if (is_table_array(v)) {
            var empty: list.List[Value] = list.new();
            for (list.to_array(as_array(v) catch empty)) |item| {
                gap(b);
                bytes.put_str(b, "[[");
                bytes.put_str(b, path);
                bytes.put_str(b, "]]\n");
                write_table(b, as_table(item) catch table(), path);
            }
        }
    }
    return;
}

/// A blank line before a header, unless nothing has been written yet.
fn gap(b: bytes.Buf) void {
    if (b.used > 0) { bytes.put_u8(b, LF); }
    return;
}

/// A table that gets a `[header]` rather than being written inline.
fn is_header_table(v: Value) bool { return false; }
fn is_header_table(v: Table) bool { return !v.braced; }

/// An array built by `[[header]]`, whose members get headers of their own.
fn is_table_array(v: Value) bool { return false; }
fn is_table_array(v: Arr) bool { return !v.literal; }

fn join_key(prefix: str, key: str) str {
    if (text.len(prefix) == 0) { return write_key(key); }
    return text.concat(text.concat(prefix, "."), write_key(key));
}

/// A key as it has to be spelled: bare where TOML allows it, quoted otherwise.
pub fn write_key(key: str) str {
    const n = text.len(key);
    if (n == 0) { return "\"\""; }
    var i = 0;
    while (i < n) : (i += 1) {
        if (!is_bare(text.byte_at(key, i))) { return quoted(key); }
    }
    return key;
}

fn write_value(b: bytes.Buf, v: Value) void {
    const k = kind(v);
    if (k == KIND_STR) { bytes.put_str(b, quoted(as_str(v) catch "")); return; }
    if (k == KIND_INT) { bytes.put_str(b, text.from_int(as_int(v) catch 0)); return; }
    if (k == KIND_FLOAT) { bytes.put_str(b, float_text(as_float(v) catch 0.0)); return; }
    if (k == KIND_BOOL) {
        if (as_bool(v) catch false) { bytes.put_str(b, "true"); } else { bytes.put_str(b, "false"); }
        return;
    }
    if (k == KIND_DATE) { bytes.put_str(b, as_date(v) catch ""); return; }
    if (k == KIND_ARRAY) {
        var empty: list.List[Value] = list.new();
        bytes.put_u8(b, LBRACKET);
        var first = true;
        for (list.to_array(as_array(v) catch empty)) |item| {
            if (!first) { bytes.put_str(b, ", "); }
            first = false;
            write_value(b, item);
        }
        bytes.put_u8(b, RBRACKET);
        return;
    }
    if (k == KIND_TABLE) {
        const t = as_table(v) catch table();
        bytes.put_str(b, "{ ");
        const n = list.len(t.keys);
        var i = 0;
        while (i < n) : (i += 1) {
            if (i > 0) { bytes.put_str(b, ", "); }
            bytes.put_str(b, write_key(list.get(t.keys, i)));
            bytes.put_str(b, " = ");
            write_value(b, list.get(t.vals, i));
        }
        bytes.put_str(b, " }");
        return;
    }
    bytes.put_str(b, "\"\"");
    return;
}

/// A basic string, escaped.
fn quoted(s: str) str {
    const b = bytes.buf(text.len(s) + 2);
    bytes.put_u8(b, QUOTE);
    const n = text.len(s);
    var i = 0;
    while (i < n) : (i += 1) {
        const c = text.byte_at(s, i);
        if (c == QUOTE or c == BACKSLASH) { bytes.put_u8(b, BACKSLASH); bytes.put_u8(b, c); continue; }
        if (c == LF) { bytes.put_str(b, "\\n"); continue; }
        if (c == CR) { bytes.put_str(b, "\\r"); continue; }
        if (c == TAB) { bytes.put_str(b, "\\t"); continue; }
        if (is_control(c)) { bytes.put_str(b, escape_hex(c)); continue; }
        bytes.put_u8(b, c);
    }
    bytes.put_u8(b, QUOTE);
    return bytes.to_str(bytes.taken(b));
}

fn escape_hex(c: i64) str {
    const digits = "0123456789abcdef";
    var out = "\\u00";
    out = text.concat(out, text.from_byte(text.byte_at(digits, (c >> 4) & 15)));
    return text.concat(out, text.from_byte(text.byte_at(digits, c & 15)));
}

/// A float as TOML spells it.
///
/// `str.from_float` writes `0` for zero and `inf` for infinity, and TOML needs
/// `0.0` and `inf` -- so a number with neither a point nor an exponent in it
/// gets one, and the two names pass through.
fn float_text(f: f64) str {
    const s = text.from_float(f);
    const n = text.len(s);
    var i = 0;
    while (i < n) : (i += 1) {
        const c = text.byte_at(s, i);
        if (c == DOT or c == 101 or c == 69) { return s; }
        // `inf`, `-inf` and `nan` are written as they are.
        if (c == 105 or c == 110 or c == 97) { return s; }
    }
    return text.concat(s, ".0");
}
