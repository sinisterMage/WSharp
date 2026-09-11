// An object keeps the order its members were written in, and finds one in
// constant time once there are enough of them to be worth an index.
//
// `std/toml`'s table scans two parallel lists and says out loud that the fix
// for a large one is a map rather than a cleverer scan -- with the reason it is
// fine there being that a manifest has tens of keys. A document off a network
// makes no such promise, so `std/json` keeps the lists, for the order the
// writer needs, and builds a `map.Map[i64]` beside them the moment an object
// passes `INDEX_AT` members.
//
// Both sides of that threshold are exercised here, because it is two paths
// through `index_of` and a bug could live in either. What must be true on both
// sides: the order is the order things were written, replacing a member keeps
// its place, and a name that is not there is not found.
// expect: small-len: 8
// expect: small-walk: walked 8
// expect: small-first: 0
// expect: small-last: 7
// expect: small-missing: false
// expect: big-len: 200
// expect: big-walk: walked 200
// expect: big-before-the-threshold: 3
// expect: big-at-the-threshold: 8
// expect: big-after-the-threshold: 9
// expect: big-last: 199
// expect: big-missing: false
// expect: small-after-replacing: k0
// expect: small-len-after-replacing: 8
// expect: big-after-replacing: k0
// expect: big-len-after-replacing: 200
// expect: replaced-value: replaced
// expect: parsed-len: 200
// expect: parsed-walk: walked 200
// expect: empty-len: 0
// expect: empty-missing: false
// expect: empty-written: {}
const json = @import("std/json");
const text = @import("std/str");

fn say(name: str, what: str) void {
    print(text.concat(name, text.concat(": ", what)));
    return;
}

fn yes_no(b: bool) str { if (b) { return "true"; } return "false"; }

/// The value at `key`, or `-1` if there is none.
fn number_at(o: json.Obj, key: str) i64 {
    return json.as_int(json.get(o, key) orelse json.of_int(-1)) catch -1;
}

/// An object of `n` members named `k0`..`k<n-1>`, each holding its own number.
fn counted(n: i64) json.Obj {
    const o = json.obj();
    var i = 0;
    while (i < n) : (i += 1) {
        json.set(o, text.concat("k", text.from_int(i)), json.of_int(i));
    }
    return o;
}

/// Every member found, in order, or the first place it went wrong.
fn walk(o: json.Obj) str {
    const names = json.keys(o);
    const n = json.len(o);
    var i = 0;
    while (i < n) : (i += 1) {
        const want = text.concat("k", text.from_int(i));
        if (!text.eq(names[i], want)) { return text.concat("out of order at ", want); }
        if (number_at(o, names[i]) != i) { return text.concat("wrong value at ", want); }
    }
    return text.concat("walked ", text.from_int(n));
}

fn main() i64 {
    // Under the threshold, which is the linear scan.
    const small = counted(8);
    say("small-len", text.from_int(json.len(small)));
    say("small-walk", walk(small));
    say("small-first", text.from_int(number_at(small, "k0")));
    say("small-last", text.from_int(number_at(small, "k7")));
    say("small-missing", yes_no(json.has(small, "k8")));

    // Over it, which is the index. The ninth member is what builds it, so the
    // eight already there have to end up in it too.
    const big = counted(200);
    say("big-len", text.from_int(json.len(big)));
    say("big-walk", walk(big));
    say("big-before-the-threshold", text.from_int(number_at(big, "k3")));
    say("big-at-the-threshold", text.from_int(number_at(big, "k8")));
    say("big-after-the-threshold", text.from_int(number_at(big, "k9")));
    say("big-last", text.from_int(number_at(big, "k199")));
    say("big-missing", yes_no(json.has(big, "k200")));

    // Replacing keeps the member where it was, on both paths -- which is what
    // makes a document with a repeated name round-trip to the shape it arrived
    // in rather than with that name moved to the end.
    json.set(small, "k0", json.of_str("replaced"));
    json.set(big, "k0", json.of_str("replaced"));
    say("small-after-replacing", json.keys(small)[0]);
    say("small-len-after-replacing", text.from_int(json.len(small)));
    say("big-after-replacing", json.keys(big)[0]);
    say("big-len-after-replacing", text.from_int(json.len(big)));
    say("replaced-value", json.as_str(json.get(big, "k0") orelse json.of_null()) catch "?");

    // The same thing read rather than built: the reader calls the same `set`,
    // so a parsed object crosses the threshold exactly where a built one does.
    const doc = json.parse(json.write(json.of_obj(counted(200))));
    if (!doc.ok) { print(doc.message); return 1; }
    const read_back = json.as_obj(doc.root) catch json.obj();
    say("parsed-len", text.from_int(json.len(read_back)));
    say("parsed-walk", walk(read_back));

    // An empty object has no index and no members, and is not a special case.
    const none = json.obj();
    say("empty-len", text.from_int(json.len(none)));
    say("empty-missing", yes_no(json.has(none, "k0")));
    say("empty-written", json.write(json.of_obj(none)));
    return 0;
}
