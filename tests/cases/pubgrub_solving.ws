// PubGrub, against the worked examples its own write-up sets out. The four
// here are the ones with a solution: no conflicts, avoiding a conflict while
// deciding, performing conflict resolution, and conflict resolution with a
// partial satisfier. Each answer is the one the paper gives.
//
// The registry is in memory, so the solver is handed two closures over a table
// -- which is exactly what the tool hands it with a store behind them. A
// `Provider` is a struct of `fn` values because that is what a language with
// multiple dispatch and no interfaces has; `std/hash.Hash` is the same shape.
// expect: no conflicts: foo 1.0.0 bar 1.0.0
// expect: avoiding conflict: bar 1.1.0 foo 1.0.0
// expect: conflict resolution: foo 1.0.0
// expect: partial satisfier: target 2.0.0 foo 1.0.0
// expect: a release candidate is not a candidate: pre 1.0.0
// expect: unless it was asked for: pre 1.1.0-rc.1
const list = @import("std/list");
const pubgrub = @import("ingot/pubgrub");
const registry = @import("./modules/registry.ws");
const semver = @import("ingot/semver");
const text = @import("std/str");

fn run(name: str, r: registry.Registry) void {
    const a = pubgrub.solve(registry.provider(r), "root", semver.parse("1.0.0") orelse semver.zero());
    if (a.ok) {
        print(text.concat(name, text.concat(": ", registry.solution(a))));
    } else {
        print(text.concat(name, ": failed"));
    }
    return;
}

fn main() i64 {
    const one = registry.registry();
    registry.add(one, "root", "1.0.0", "foo ^1.0.0");
    registry.add(one, "foo", "1.0.0", "bar ^1.0.0");
    registry.add(one, "bar", "1.0.0", "");
    registry.add(one, "bar", "2.0.0", "");
    run("no conflicts", one);

    // `foo 1.1.0` would need `bar ^2.0.0`, which the root forbids, so the
    // solver has to notice before deciding rather than after.
    const two = registry.registry();
    registry.add(two, "root", "1.0.0", "foo ^1.0.0, bar ^1.0.0");
    registry.add(two, "foo", "1.1.0", "bar ^2.0.0");
    registry.add(two, "foo", "1.0.0", "");
    registry.add(two, "bar", "1.0.0", "");
    registry.add(two, "bar", "1.1.0", "");
    registry.add(two, "bar", "2.0.0", "");
    run("avoiding conflict", two);

    // Here it does not: `foo 2.0.0` is chosen, found impossible, and the
    // clause learned from that is what rules it out for good.
    const three = registry.registry();
    registry.add(three, "root", "1.0.0", "foo >=1.0.0");
    registry.add(three, "foo", "2.0.0", "bar ^1.0.0");
    registry.add(three, "foo", "1.0.0", "");
    registry.add(three, "bar", "1.0.0", "foo ^1.0.0");
    run("conflict resolution", three);

    // The case the algorithm is subtle for: the satisfier covers only part of
    // the term, so resolution has to carry the rest into the new clause.
    const four = registry.registry();
    registry.add(four, "root", "1.0.0", "foo ^1.0.0, target ^2.0.0");
    registry.add(four, "foo", "1.1.0", "left ^1.0.0, right ^1.0.0");
    registry.add(four, "foo", "1.0.0", "");
    registry.add(four, "left", "1.0.0", "shared >=1.0.0");
    registry.add(four, "right", "1.0.0", "shared <2.0.0");
    registry.add(four, "shared", "2.0.0", "");
    registry.add(four, "shared", "1.0.0", "target ^1.0.0");
    registry.add(four, "target", "2.0.0", "");
    registry.add(four, "target", "1.0.0", "");
    run("partial satisfier", four);

    // A pre-release is not a candidate unless it was asked for. The set
    // algebra stays honest that `1.1.0-rc.1` is less than `1.1.0` and inside
    // `^1.0.0`; this is the policy on top of it.
    const five = registry.registry();
    registry.add(five, "root", "1.0.0", "pre ^1.0.0");
    registry.add(five, "pre", "1.0.0", "");
    registry.add(five, "pre", "1.1.0-rc.1", "");
    run("a release candidate is not a candidate", five);

    const six = registry.registry();
    registry.add(six, "root", "1.0.0", "pre >=1.1.0-rc.1");
    registry.add(six, "pre", "1.0.0", "");
    registry.add(six, "pre", "1.1.0-rc.1", "");
    run("unless it was asked for", six);
    return 0;
}
