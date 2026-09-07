// Semantic versions, and sets of them.
//
// The versions are the easy half. The sets are the half PubGrub needs, and the
// reason they are **unions of intervals** rather than a predicate is one
// operation: the solver takes the *complement* of a requirement constantly --
// "not foo ^1.0.0" is a term it derives and reasons about -- and a predicate
// cannot be complemented into something you can then ask for the best version
// of.
//
// An interval carries whether each end is included, rather than being
// half-open. A half-open representation needs a successor function, and a
// version has no successor: there are infinitely many pre-releases between
// `1.0.0` and `1.0.1`.
const array = @import("std/array");
const list = @import("std/list");
const text = @import("std/str");

// ---------------------------------------------------------------------------
// Versions
// ---------------------------------------------------------------------------

/// `major.minor.patch`, with the two suffixes semver 2.0.0 defines.
///
/// `build` is carried so that a version round-trips through the lockfile, and
/// is ignored in every comparison -- which is what the specification says and
/// is the one rule people are surprised by.
pub const Version = struct { major: i64, minor: i64, patch: i64, pre: str, build: str };

pub fn version(major: i64, minor: i64, patch: i64) Version {
    return Version{ .major = major, .minor = minor, .patch = patch, .pre = "", .build = "" };
}

pub fn zero() Version { return version(0, 0, 0); }

/// Read `1.2.3`, `1.2.3-rc.1`, `1.2.3+build.5`, or nothing.
///
/// Strict: three numbers, no leading zeros, and the two suffixes in that order.
/// `1.2` is not a version -- it is a *requirement*, and `requirement` reads it.
pub fn parse(s: str) ?Version {
    var rest = s;
    var build = "";
    const plus = text.find(rest, "+");
    if (plus >= 0) {
        build = text.substr(rest, plus + 1, text.len(rest));
        rest = text.substr(rest, 0, plus);
        if (!identifiers_ok(build, true)) { return null; }
    }
    var pre = "";
    const dash = text.find(rest, "-");
    if (dash >= 0) {
        pre = text.substr(rest, dash + 1, text.len(rest));
        rest = text.substr(rest, 0, dash);
        if (!identifiers_ok(pre, false)) { return null; }
    }
    const parts = text.split(rest, ".");
    if (array.len(parts) != 3) { return null; }
    const major = number(parts[0]) orelse return null;
    const minor = number(parts[1]) orelse return null;
    const patch = number(parts[2]) orelse return null;
    return Version{ .major = major, .minor = minor, .patch = patch, .pre = pre, .build = build };
}

pub fn render(v: Version) str {
    var out = text.from_int(v.major);
    out = text.concat(out, text.concat(".", text.from_int(v.minor)));
    out = text.concat(out, text.concat(".", text.from_int(v.patch)));
    if (text.len(v.pre) > 0) { out = text.concat(out, text.concat("-", v.pre)); }
    if (text.len(v.build) > 0) { out = text.concat(out, text.concat("+", v.build)); }
    return out;
}

/// A numeric identifier: digits, and no leading zero unless it is a lone zero.
fn number(s: str) ?i64 {
    const n = text.len(s);
    if (n == 0) { return null; }
    if (n > 1 and text.byte_at(s, 0) == 48) { return null; }
    var i = 0;
    while (i < n) : (i += 1) {
        if (!is_digit(text.byte_at(s, i))) { return null; }
    }
    return text.parse_int(s) catch return null;
}

fn is_digit(c: i64) bool { return c >= 48 and c <= 57; }

fn is_identifier_byte(c: i64) bool {
    return is_digit(c) or (c >= 97 and c <= 122) or (c >= 65 and c <= 90) or c == 45;
}

/// Dot-separated identifiers, each non-empty and alphanumeric-or-hyphen.
///
/// A numeric identifier in a *pre-release* may not have a leading zero; build
/// metadata has no such rule, because it is never compared.
fn identifiers_ok(s: str, is_build: bool) bool {
    const parts = text.split(s, ".");
    const n = array.len(parts);
    if (n == 0) { return false; }
    var i = 0;
    while (i < n) : (i += 1) {
        const part = parts[i];
        const m = text.len(part);
        if (m == 0) { return false; }
        var j = 0;
        var numeric = true;
        while (j < m) : (j += 1) {
            const c = text.byte_at(part, j);
            if (!is_identifier_byte(c)) { return false; }
            if (!is_digit(c)) { numeric = false; }
        }
        if (numeric and !is_build and m > 1 and text.byte_at(part, 0) == 48) { return false; }
    }
    return true;
}

/// -1, 0 or 1. Build metadata is not compared, per the specification.
pub fn cmp(a: Version, b: Version) i64 {
    if (a.major != b.major) { return sign(a.major - b.major); }
    if (a.minor != b.minor) { return sign(a.minor - b.minor); }
    if (a.patch != b.patch) { return sign(a.patch - b.patch); }
    return compare_pre(a.pre, b.pre);
}

pub fn eq(a: Version, b: Version) bool { return cmp(a, b) == 0; }
pub fn less(a: Version, b: Version) bool { return cmp(a, b) < 0; }

/// A version with a pre-release is *lower* than the same one without, which is
/// the rule that makes `1.0.0-rc.1` come before `1.0.0` rather than after it.
///
/// Then identifier by identifier: numbers numerically, anything else by bytes,
/// a number below anything else, and a shorter run of identifiers below a
/// longer one that agrees so far.
fn compare_pre(a: str, b: str) i64 {
    const empty_a = text.len(a) == 0;
    const empty_b = text.len(b) == 0;
    if (empty_a and empty_b) { return 0; }
    if (empty_a) { return 1; }
    if (empty_b) { return -1; }
    const xs = text.split(a, ".");
    const ys = text.split(b, ".");
    const n = min(array.len(xs), array.len(ys));
    var i = 0;
    while (i < n) : (i += 1) {
        const order = compare_identifier(xs[i], ys[i]);
        if (order != 0) { return order; }
    }
    return sign(array.len(xs) - array.len(ys));
}

fn compare_identifier(a: str, b: str) i64 {
    const na = numeric_value(a);
    const nb = numeric_value(b);
    if (na >= 0 and nb >= 0) { return sign(na - nb); }
    if (na >= 0) { return -1; }
    if (nb >= 0) { return 1; }
    return compare_bytes(a, b);
}

/// The identifier's value if every byte is a digit, and -1 otherwise.
fn numeric_value(s: str) i64 {
    const n = text.len(s);
    if (n == 0) { return -1; }
    var i = 0;
    while (i < n) : (i += 1) {
        if (!is_digit(text.byte_at(s, i))) { return -1; }
    }
    return text.parse_int(s) catch -1;
}

fn compare_bytes(a: str, b: str) i64 {
    const n = min(text.len(a), text.len(b));
    var i = 0;
    while (i < n) : (i += 1) {
        const x = text.byte_at(a, i);
        const y = text.byte_at(b, i);
        if (x != y) { return sign(x - y); }
    }
    return sign(text.len(a) - text.len(b));
}

fn sign(n: i64) i64 {
    if (n < 0) { return -1; }
    if (n > 0) { return 1; }
    return 0;
}

fn min(a: i64, b: i64) i64 { if (a < b) { return a; } return b; }

// ---------------------------------------------------------------------------
// Sets of versions
// ---------------------------------------------------------------------------

/// How an interval ends.
pub const UNBOUNDED = 0;
pub const INCLUSIVE = 1;
pub const EXCLUSIVE = 2;

pub const Bound = struct { v: Version, kind: i64 };

/// One run of versions, from `lo` up to `hi`.
pub const Interval = struct { lo: Bound, hi: Bound };

/// A set of versions, as intervals that are sorted, disjoint and not adjacent.
///
/// Kept in that form by construction rather than checked, because every
/// operation below either produces it directly or goes through `normalise`.
/// Two ranges that hold the same versions therefore have the same intervals,
/// which is what lets `same` be a walk rather than a double subset test.
pub const Range = struct { parts: list.List[Interval] };

pub fn open() Bound { return Bound{ .v = zero(), .kind = UNBOUNDED }; }
pub fn at_least(v: Version) Bound { return Bound{ .v = v, .kind = INCLUSIVE }; }
pub fn above(v: Version) Bound { return Bound{ .v = v, .kind = EXCLUSIVE }; }

pub fn empty() Range { var parts: list.List[Interval] = list.new(); return Range{ .parts = parts }; }

pub fn any() Range {
    return one(Interval{ .lo = open(), .hi = open() });
}

pub fn exactly(v: Version) Range {
    return one(Interval{ .lo = at_least(v), .hi = Bound{ .v = v, .kind = INCLUSIVE } });
}

pub fn between(lo: Bound, hi: Bound) Range {
    if (!inhabited(lo, hi)) { return empty(); }
    return one(Interval{ .lo = lo, .hi = hi });
}

fn one(i: Interval) Range {
    var parts: list.List[Interval] = list.new();
    list.push(parts, i);
    return Range{ .parts = parts };
}

pub fn is_empty(r: Range) bool { return list.len(r.parts) == 0; }

pub fn is_any(r: Range) bool {
    if (list.len(r.parts) != 1) { return false; }
    const i = list.get(r.parts, 0);
    return i.lo.kind == UNBOUNDED and i.hi.kind == UNBOUNDED;
}

pub fn contains(r: Range, v: Version) bool {
    const n = list.len(r.parts);
    var i = 0;
    while (i < n) : (i += 1) {
        if (holds(list.get(r.parts, i), v)) { return true; }
    }
    return false;
}

fn holds(i: Interval, v: Version) bool {
    if (i.lo.kind != UNBOUNDED) {
        const order = cmp(v, i.lo.v);
        if (order < 0) { return false; }
        if (order == 0 and i.lo.kind == EXCLUSIVE) { return false; }
    }
    if (i.hi.kind != UNBOUNDED) {
        const order = cmp(v, i.hi.v);
        if (order > 0) { return false; }
        if (order == 0 and i.hi.kind == EXCLUSIVE) { return false; }
    }
    return true;
}

/// Whether an interval with these ends holds anything at all.
fn inhabited(lo: Bound, hi: Bound) bool {
    if (lo.kind == UNBOUNDED or hi.kind == UNBOUNDED) { return true; }
    const order = cmp(lo.v, hi.v);
    if (order < 0) { return true; }
    if (order > 0) { return false; }
    return lo.kind == INCLUSIVE and hi.kind == INCLUSIVE;
}

/// Which of two *lower* bounds starts earlier. Unbounded starts earliest, and
/// at the same version an inclusive bound starts before an exclusive one.
fn lower_cmp(a: Bound, b: Bound) i64 {
    if (a.kind == UNBOUNDED and b.kind == UNBOUNDED) { return 0; }
    if (a.kind == UNBOUNDED) { return -1; }
    if (b.kind == UNBOUNDED) { return 1; }
    const order = cmp(a.v, b.v);
    if (order != 0) { return order; }
    if (a.kind == b.kind) { return 0; }
    if (a.kind == INCLUSIVE) { return -1; }
    return 1;
}

/// Which of two *upper* bounds ends earlier. Unbounded ends last, and at the
/// same version an exclusive bound ends before an inclusive one.
fn upper_cmp(a: Bound, b: Bound) i64 {
    if (a.kind == UNBOUNDED and b.kind == UNBOUNDED) { return 0; }
    if (a.kind == UNBOUNDED) { return 1; }
    if (b.kind == UNBOUNDED) { return -1; }
    const order = cmp(a.v, b.v);
    if (order != 0) { return order; }
    if (a.kind == b.kind) { return 0; }
    if (a.kind == EXCLUSIVE) { return -1; }
    return 1;
}

/// Everything in both.
pub fn intersect(a: Range, b: Range) Range {
    var parts: list.List[Interval] = list.new();
    const na = list.len(a.parts);
    const nb = list.len(b.parts);
    var i = 0;
    while (i < na) : (i += 1) {
        const x = list.get(a.parts, i);
        var j = 0;
        while (j < nb) : (j += 1) {
            const y = list.get(b.parts, j);
            const lo = later(x.lo, y.lo);
            const hi = earlier(x.hi, y.hi);
            if (inhabited(lo, hi)) { list.push(parts, Interval{ .lo = lo, .hi = hi }); }
        }
    }
    return normalise(parts);
}

fn later(a: Bound, b: Bound) Bound { if (lower_cmp(a, b) >= 0) { return a; } return b; }
fn earlier(a: Bound, b: Bound) Bound { if (upper_cmp(a, b) <= 0) { return a; } return b; }

/// Everything in either.
pub fn unite(a: Range, b: Range) Range {
    var parts: list.List[Interval] = list.new();
    for (list.to_array(a.parts)) |i| { list.push(parts, i); }
    for (list.to_array(b.parts)) |i| { list.push(parts, i); }
    return normalise(parts);
}

/// Everything else.
///
/// The operation intervals exist for: a solver's terms are as often "not this
/// range" as "this range", and it has to be able to ask what versions such a
/// term allows.
pub fn complement(r: Range) Range {
    if (is_empty(r)) { return any(); }
    var parts: list.List[Interval] = list.new();
    const n = list.len(r.parts);
    const first = list.get(r.parts, 0);
    if (first.lo.kind != UNBOUNDED) {
        list.push(parts, Interval{ .lo = open(), .hi = flip(first.lo) });
    }
    var i = 0;
    while (i < n - 1) : (i += 1) {
        const here = list.get(r.parts, i);
        const next = list.get(r.parts, i + 1);
        list.push(parts, Interval{ .lo = flip(here.hi), .hi = flip(next.lo) });
    }
    const last = list.get(r.parts, n - 1);
    if (last.hi.kind != UNBOUNDED) {
        list.push(parts, Interval{ .lo = flip(last.hi), .hi = open() });
    }
    return normalise(parts);
}

/// The bound that starts where this one stops, or stops where it starts:
/// inclusive becomes exclusive and back.
fn flip(b: Bound) Bound {
    if (b.kind == INCLUSIVE) { return Bound{ .v = b.v, .kind = EXCLUSIVE }; }
    return Bound{ .v = b.v, .kind = INCLUSIVE };
}

/// Everything in `a` that is not in `b`.
pub fn without(a: Range, b: Range) Range { return intersect(a, complement(b)); }

pub fn subset(a: Range, b: Range) bool { return is_empty(without(a, b)); }

pub fn same(a: Range, b: Range) bool {
    const n = list.len(a.parts);
    if (n != list.len(b.parts)) { return false; }
    var i = 0;
    while (i < n) : (i += 1) {
        const x = list.get(a.parts, i);
        const y = list.get(b.parts, i);
        if (lower_cmp(x.lo, y.lo) != 0 or upper_cmp(x.hi, y.hi) != 0) { return false; }
    }
    return true;
}

/// Sorted, with overlapping and touching intervals run together.
///
/// Two intervals *touch* when one ends exactly where the other begins and at
/// least one end includes that version: `<=1.0.0` and `>1.0.0` cover
/// everything between them, and leaving them apart would make `same` say two
/// equal sets are different.
fn normalise(parts: list.List[Interval]) Range {
    const sorted = sort_intervals(parts);
    var out: list.List[Interval] = list.new();
    for (list.to_array(sorted)) |i| {
        if (list.len(out) == 0) { list.push(out, i); continue; }
        const back = list.get(out, list.len(out) - 1);
        if (touching(back, i)) {
            list.set(out, list.len(out) - 1,
                Interval{ .lo = back.lo, .hi = later_upper(back.hi, i.hi) });
            continue;
        }
        list.push(out, i);
    }
    return Range{ .parts = out };
}

fn later_upper(a: Bound, b: Bound) Bound { if (upper_cmp(a, b) >= 0) { return a; } return b; }

fn touching(before: Interval, after: Interval) bool {
    if (before.hi.kind == UNBOUNDED or after.lo.kind == UNBOUNDED) { return true; }
    const order = cmp(before.hi.v, after.lo.v);
    if (order > 0) { return true; }
    if (order < 0) { return false; }
    // The same version: they meet unless both ends exclude it.
    return before.hi.kind == INCLUSIVE or after.lo.kind == INCLUSIVE;
}

fn sort_intervals(parts: list.List[Interval]) list.List[Interval] {
    const n = list.len(parts);
    var out: list.List[Interval] = list.new();
    for (list.to_array(parts)) |i| { list.push(out, i); }
    var i = 1;
    while (i < n) : (i += 1) {
        const v = list.get(out, i);
        var j = i;
        while (j > 0 and lower_cmp(v.lo, list.get(out, j - 1).lo) < 0) : (j -= 1) {
            list.set(out, j, list.get(out, j - 1));
        }
        list.set(out, j, v);
    }
    return out;
}

/// Whether any bound in this set names a pre-release.
///
/// The rule everyone eventually needs and nobody writes down first: a
/// pre-release is not a candidate unless it was asked for. Keeping that out of
/// the set algebra and putting it here is what lets the algebra stay honest --
/// `2.0.0-rc.1` really is less than `2.0.0` and really is inside
/// `>=1.0.0 <2.0.0` -- while the resolver still refuses to hand somebody a
/// release candidate they did not name.
pub fn mentions_prerelease(r: Range) bool {
    for (list.to_array(r.parts)) |i| {
        if (i.lo.kind != UNBOUNDED and text.len(i.lo.v.pre) > 0) { return true; }
        if (i.hi.kind != UNBOUNDED and text.len(i.hi.v.pre) > 0) { return true; }
    }
    return false;
}

/// How a set reads in a message. The solver's explanations are made of these,
/// so it is worth their being what a person would have written.
pub fn show(r: Range) str {
    if (is_empty(r)) { return "no version"; }
    if (is_any(r)) { return "any version"; }
    var out = "";
    var first = true;
    for (list.to_array(r.parts)) |i| {
        if (!first) { out = text.concat(out, " or "); }
        first = false;
        out = text.concat(out, show_interval(i));
    }
    return out;
}

fn show_interval(i: Interval) str {
    if (i.lo.kind == UNBOUNDED and i.hi.kind == UNBOUNDED) { return "any version"; }
    if (i.lo.kind == INCLUSIVE and i.hi.kind == INCLUSIVE and eq(i.lo.v, i.hi.v)) {
        return render(i.lo.v);
    }
    if (i.lo.kind == UNBOUNDED) { return text.concat(upper_word(i.hi), render(i.hi.v)); }
    if (i.hi.kind == UNBOUNDED) { return text.concat(lower_word(i.lo), render(i.lo.v)); }
    return text.concat(text.concat(lower_word(i.lo), render(i.lo.v)),
        text.concat(" ", text.concat(upper_word(i.hi), render(i.hi.v))));
}

fn lower_word(b: Bound) str { if (b.kind == INCLUSIVE) { return ">="; } return ">"; }
fn upper_word(b: Bound) str { if (b.kind == INCLUSIVE) { return "<="; } return "<"; }

// ---------------------------------------------------------------------------
// Requirements
// ---------------------------------------------------------------------------

/// Read what a manifest writes: `^1.2.3`, `~1.2`, `>=1.0.0, <2.0.0`, `1.2.3`,
/// `=1.2.3`, `*`.
///
/// A comma is an intersection, which is the one piece of syntax here that is
/// not a shorthand for one. A bare version means `^`, as Cargo has it: the
/// common case is "this or anything compatible with it", and spelling that out
/// every time is how a manifest ends up pinned by accident.
pub fn requirement(s: str) ?Range {
    const trimmed = text.trim(s);
    if (text.len(trimmed) == 0) { return null; }
    var out = any();
    for (text.split(trimmed, ",")) |piece| {
        const part = clause(text.trim(piece)) orelse return null;
        out = intersect(out, part);
    }
    return out;
}

fn clause(s: str) ?Range {
    if (text.eq(s, "*")) { return any(); }
    if (text.len(s) == 0) { return null; }
    const c = text.byte_at(s, 0);
    if (c == 94) { return caret(text.substr(s, 1, text.len(s))); }
    if (c == 126) { return tilde(text.substr(s, 1, text.len(s))); }
    if (c == 61) { return exact(text.substr(s, 1, text.len(s))); }
    if (c == 62) {
        if (text.byte_at(s, 1) == 61) {
            const v = parse(text.trim(text.substr(s, 2, text.len(s)))) orelse return null;
            return between(at_least(v), open());
        }
        const v = parse(text.trim(text.substr(s, 1, text.len(s)))) orelse return null;
        return between(above(v), open());
    }
    if (c == 60) {
        if (text.byte_at(s, 1) == 61) {
            const v = parse(text.trim(text.substr(s, 2, text.len(s)))) orelse return null;
            return between(open(), Bound{ .v = v, .kind = INCLUSIVE });
        }
        const v = parse(text.trim(text.substr(s, 1, text.len(s)))) orelse return null;
        return between(open(), Bound{ .v = v, .kind = EXCLUSIVE });
    }
    return caret(s);
}

fn exact(s: str) ?Range {
    const v = parse(text.trim(s)) orelse return null;
    return exactly(v);
}

/// `^1.2.3` is everything up to the next change that may break: the next major
/// version, or -- below 1.0.0, where the major number is not carrying that
/// meaning yet -- the next minor or patch one.
fn caret(s: str) ?Range {
    const p = partial(text.trim(s)) orelse return null;
    const lo = p.v;
    return between(at_least(lo), Bound{ .v = caret_end(p), .kind = EXCLUSIVE });
}

/// The next version that may break: increment the leftmost number that is not
/// zero, and if every number written is zero, the last one that was written.
///
/// `^0.2.3` is `<0.3.0` and `^0.0.3` is `<0.0.4`, because below 1.0.0 the major
/// number is not yet carrying the meaning that makes `^` worth having.
fn caret_end(p: Partial) Version {
    const lo = p.v;
    if (lo.major > 0) { return version(lo.major + 1, 0, 0); }
    if (p.given == 1) { return version(1, 0, 0); }
    if (lo.minor > 0) { return version(0, lo.minor + 1, 0); }
    if (p.given == 2) { return version(0, 1, 0); }
    return version(0, 0, lo.patch + 1);
}

/// `~1.2.3` is everything up to the next minor version; `~1` is up to the next
/// major one, because there is no minor number to hold fixed.
fn tilde(s: str) ?Range {
    const p = partial(text.trim(s)) orelse return null;
    const lo = p.v;
    var hi = version(lo.major + 1, 0, 0);
    if (p.given >= 2) { hi = version(lo.major, lo.minor + 1, 0); }
    return between(at_least(lo), Bound{ .v = hi, .kind = EXCLUSIVE });
}

/// A version with some of its numbers left off, and how many were written.
const Partial = struct { v: Version, given: i64 };

fn partial(s: str) ?Partial {
    if (text.find(s, "-") >= 0 or text.find(s, "+") >= 0) {
        const whole = parse(s) orelse return null;
        return Partial{ .v = whole, .given = 3 };
    }
    const parts = text.split(s, ".");
    const n = array.len(parts);
    if (n < 1 or n > 3) { return null; }
    const major = number(parts[0]) orelse return null;
    var minor = 0;
    var patch = 0;
    if (n >= 2) { minor = number(parts[1]) orelse return null; }
    if (n >= 3) { patch = number(parts[2]) orelse return null; }
    return Partial{ .v = version(major, minor, patch), .given = n };
}
