// Semantic versions, and sets of them.
//
// The order is semver 2.0.0's own worked example, in the order the
// specification lists it. The sets are what PubGrub needs, and are unions of
// intervals rather than a predicate for one reason: the solver takes
// complements constantly, and a predicate cannot be complemented into
// something you can then ask for the best version of.
// expect: 1.2.3 < 1.2.4
// expect: 1.2.3 = 1.2.3
// expect: 1.0.0-alpha < 1.0.0-alpha.1
// expect: 1.0.0-alpha.1 < 1.0.0-alpha.beta
// expect: 1.0.0-alpha.beta < 1.0.0-beta
// expect: 1.0.0-beta < 1.0.0-beta.2
// expect: 1.0.0-beta.2 < 1.0.0-beta.11
// expect: 1.0.0-beta.11 < 1.0.0-rc.1
// expect: 1.0.0-rc.1 < 1.0.0
// expect: 1.0.0+a = 1.0.0+b
// expect: refused 1.2
// expect: refused 01.2.3
// expect: refused 1.2.3-
// expect: refused 1.2.3-01
// expect: refused 1.2.3-a_b
// expect: ^1.2.3 -> >=1.2.3 <2.0.0
// expect: ^1.2 -> >=1.2.0 <2.0.0
// expect: ^1 -> >=1.0.0 <2.0.0
// expect: ^0.2.3 -> >=0.2.3 <0.3.0
// expect: ^0.2 -> >=0.2.0 <0.3.0
// expect: ^0.0.3 -> >=0.0.3 <0.0.4
// expect: ^0.0 -> >=0.0.0 <0.1.0
// expect: ^0 -> >=0.0.0 <1.0.0
// expect: ^0.0.0 -> >=0.0.0 <0.0.1
// expect: ~1.2.3 -> >=1.2.3 <1.3.0
// expect: ~1.2 -> >=1.2.0 <1.3.0
// expect: ~1 -> >=1.0.0 <2.0.0
// expect: 1.2.3 -> >=1.2.3 <2.0.0
// expect: =1.2.3 -> 1.2.3
// expect: * -> any version
// expect: >=1.0.0, <2.0.0 -> >=1.0.0 <2.0.0
// expect: >1.0.0 -> >1.0.0
// expect: <=1.0.0 -> <=1.0.0
// expect: >=1.0.0, <1.0.0 -> no version
// expect: intersect: >=1.5.0 <2.0.0
// expect: unite: >=1.0.0
// expect: complement: <1.0.0 or >=2.0.0
// expect: without: >=1.0.0 <1.5.0
// expect: a set is inside itself: yes
// expect: a set and its complement cover everything: yes
// expect: and share nothing: yes
// expect: 1.9.9 is in ^1.0.0: yes
// expect: 2.0.0 is not: yes
// expect: touching intervals join up: yes
// expect: ^1.0.0 names no pre-release: yes
// expect: >=1.1.0-rc.1 does: yes
const semver = @import("ingot/semver");
const text = @import("std/str");

fn order(a: str, b: str) void {
    const x = semver.parse(a) orelse { print(text.concat("unreadable: ", a)); return; };
    const y = semver.parse(b) orelse { print(text.concat("unreadable: ", b)); return; };
    const c = semver.cmp(x, y);
    var sign = "=";
    if (c < 0) { sign = "<"; }
    if (c > 0) { sign = ">"; }
    print(text.concat(a, text.concat(" ", text.concat(sign, text.concat(" ", b)))));
    return;
}

fn refused(s: str) void {
    if (semver.parse(s)) |v| {
        print(text.concat("ACCEPTED ", s));
    } else {
        print(text.concat("refused ", s));
    }
    return;
}

fn req(s: str) void {
    const r = semver.requirement(s) orelse { print(text.concat(s, " -> unreadable")); return; };
    print(text.concat(s, text.concat(" -> ", semver.show(r))));
    return;
}

fn yes(name: str, b: bool) void {
    if (b) { print(text.concat(name, ": yes")); } else { print(text.concat(name, ": NO")); }
    return;
}

fn main() i64 {
    order("1.2.3", "1.2.4");
    order("1.2.3", "1.2.3");
    // The precedence example from the specification, in its own order.
    order("1.0.0-alpha", "1.0.0-alpha.1");
    order("1.0.0-alpha.1", "1.0.0-alpha.beta");
    order("1.0.0-alpha.beta", "1.0.0-beta");
    order("1.0.0-beta", "1.0.0-beta.2");
    // Numerically, not as text: 11 comes after 2.
    order("1.0.0-beta.2", "1.0.0-beta.11");
    order("1.0.0-beta.11", "1.0.0-rc.1");
    // A pre-release is lower than the release it leads to.
    order("1.0.0-rc.1", "1.0.0");
    // Build metadata is not compared at all.
    order("1.0.0+a", "1.0.0+b");

    refused("1.2");
    refused("01.2.3");
    refused("1.2.3-");
    refused("1.2.3-01");
    refused("1.2.3-a_b");

    // The caret table, which is Cargo's.
    req("^1.2.3");
    req("^1.2");
    req("^1");
    req("^0.2.3");
    req("^0.2");
    req("^0.0.3");
    req("^0.0");
    req("^0");
    req("^0.0.0");
    req("~1.2.3");
    req("~1.2");
    req("~1");
    // A bare version is a caret, not a pin: the common case is "this or
    // anything compatible", and spelling it out every time is how a manifest
    // ends up pinned by accident.
    req("1.2.3");
    req("=1.2.3");
    req("*");
    req(">=1.0.0, <2.0.0");
    req(">1.0.0");
    req("<=1.0.0");
    req(">=1.0.0, <1.0.0");

    const a = semver.requirement("^1.0.0") orelse semver.empty();
    const b = semver.requirement(">=1.5.0") orelse semver.empty();
    print(text.concat("intersect: ", semver.show(semver.intersect(a, b))));
    print(text.concat("unite: ", semver.show(semver.unite(a, b))));
    print(text.concat("complement: ", semver.show(semver.complement(a))));
    print(text.concat("without: ", semver.show(semver.without(a, b))));

    yes("a set is inside itself", semver.subset(a, a));
    yes("a set and its complement cover everything",
        semver.same(semver.unite(a, semver.complement(a)), semver.any()));
    yes("and share nothing", semver.is_empty(semver.intersect(a, semver.complement(a))));
    yes("1.9.9 is in ^1.0.0", semver.contains(a, semver.parse("1.9.9") orelse semver.zero()));
    yes("2.0.0 is not", !semver.contains(a, semver.parse("2.0.0") orelse semver.zero()));

    // Touching intervals run together, so two spellings of one set are one set
    // -- which is what lets equality be a walk rather than two subset tests.
    const below = semver.requirement("<=1.0.0") orelse semver.empty();
    const above = semver.requirement(">1.0.0") orelse semver.empty();
    yes("touching intervals join up", semver.same(semver.unite(below, above), semver.any()));

    yes("^1.0.0 names no pre-release", !semver.mentions_prerelease(a));
    yes(">=1.1.0-rc.1 does",
        semver.mentions_prerelease(semver.requirement(">=1.1.0-rc.1") orelse semver.empty()));
    return 0;
}
