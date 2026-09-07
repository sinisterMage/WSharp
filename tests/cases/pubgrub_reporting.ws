// What the solver says when there is no answer, which is the whole reason for
// choosing PubGrub. "Unsatisfiable" tells nobody anything; a derivation names
// the line of the manifest to change.
//
// Every incompatibility the search derives keeps the two it came from, so the
// failure is a graph and the report is a walk of it. These are the write-up's
// own linear and branching examples.
// expect: linear:
// expect: Because no versions of foo match >1.0.0 <2.0.0 and foo 1.0.0 depends on bar >=2.0.0 <3.0.0, foo >=1.0.0 <2.0.0 depends on bar >=2.0.0 <3.0.0.
// expect: Because foo >=1.0.0 <2.0.0 depends on bar >=2.0.0 <3.0.0 and bar 2.0.0 depends on baz >=3.0.0 <4.0.0, one of foo >=1.0.0 <2.0.0, baz >=3.0.0 <4.0.0, bar >2.0.0 <3.0.0 must be left out.
// expect: Because one of foo >=1.0.0 <2.0.0, baz >=3.0.0 <4.0.0, bar >2.0.0 <3.0.0 must be left out and no versions of bar match >2.0.0 <3.0.0, foo >=1.0.0 <2.0.0 depends on baz >=3.0.0 <4.0.0.
// expect: Because foo >=1.0.0 <2.0.0 depends on baz >=3.0.0 <4.0.0 and root 1.0.0 depends on foo >=1.0.0 <2.0.0, root 1.0.0 depends on baz >=3.0.0 <4.0.0.
// expect: Because root 1.0.0 depends on baz >=3.0.0 <4.0.0 and root 1.0.0 depends on baz >=1.0.0 <2.0.0, version solving failed.
// expect: branching:
// expect: Because a 1.0.0 depends on b >=2.0.0 <3.0.0 and foo 1.0.0 depends on a >=1.0.0 <2.0.0, one of b >=2.0.0 <3.0.0, foo 1.0.0, a >1.0.0 <2.0.0 must be left out.
// expect: Because one of b >=2.0.0 <3.0.0, foo 1.0.0, a >1.0.0 <2.0.0 must be left out and foo 1.0.0 depends on b >=1.0.0 <2.0.0, foo 1.0.0 depends on a >1.0.0 <2.0.0.
// expect: Because no versions of foo match >1.0.0 <1.1.0 or >1.1.0 <2.0.0 and foo 1.0.0 depends on a >1.0.0 <2.0.0, foo >=1.0.0 <1.1.0 or >1.1.0 <2.0.0 depends on a >1.0.0 <2.0.0.
// expect: Because foo >=1.0.0 <1.1.0 or >1.1.0 <2.0.0 depends on a >1.0.0 <2.0.0 and no versions of a match >1.0.0 <2.0.0, foo >=1.0.0 <1.1.0 or >1.1.0 <2.0.0 cannot be used.
// expect: Because x 1.0.0 depends on y >=2.0.0 <3.0.0 and foo 1.1.0 depends on x >=1.0.0 <2.0.0, one of y >=2.0.0 <3.0.0, foo 1.1.0, x >1.0.0 <2.0.0 must be left out.
// expect: Because one of y >=2.0.0 <3.0.0, foo 1.1.0, x >1.0.0 <2.0.0 must be left out and foo 1.1.0 depends on y >=1.0.0 <2.0.0, foo 1.1.0 depends on x >1.0.0 <2.0.0.
// expect: Because foo >=1.0.0 <1.1.0 or >1.1.0 <2.0.0 cannot be used and foo 1.1.0 depends on x >1.0.0 <2.0.0, foo >=1.0.0 <2.0.0 depends on x >1.0.0 <2.0.0.
// expect: Because foo >=1.0.0 <2.0.0 depends on x >1.0.0 <2.0.0 and no versions of x match >1.0.0 <2.0.0, foo >=1.0.0 <2.0.0 cannot be used.
// expect: Because foo >=1.0.0 <2.0.0 cannot be used and root 1.0.0 depends on foo >=1.0.0 <2.0.0, version solving failed.
// expect: nothing there:
// expect: Because no versions of missing match >=1.0.0 <2.0.0 and root 1.0.0 depends on missing >=1.0.0 <2.0.0, version solving failed.
// expect: nothing in range:
// expect: Because no versions of foo match >=2.0.0 <3.0.0 and root 1.0.0 depends on foo >=2.0.0 <3.0.0, version solving failed.
const list = @import("std/list");
const pubgrub = @import("ingot/pubgrub");
const registry = @import("./modules/registry.ws");
const semver = @import("ingot/semver");
const text = @import("std/str");

fn run(name: str, r: registry.Registry) void {
    const a = pubgrub.solve(registry.provider(r), "root", semver.parse("1.0.0") orelse semver.zero());
    print(text.concat(name, ":"));
    if (a.ok) {
        print(text.concat("  UNEXPECTEDLY SOLVED: ", registry.solution(a)));
        return;
    }
    print(a.report);
    return;
}

fn main() i64 {
    const one = registry.registry();
    registry.add(one, "root", "1.0.0", "foo ^1.0.0, baz ^1.0.0");
    registry.add(one, "foo", "1.0.0", "bar ^2.0.0");
    registry.add(one, "bar", "2.0.0", "baz ^3.0.0");
    registry.add(one, "baz", "1.0.0", "");
    registry.add(one, "baz", "3.0.0", "");
    run("linear", one);

    const two = registry.registry();
    registry.add(two, "root", "1.0.0", "foo ^1.0.0");
    registry.add(two, "foo", "1.0.0", "a ^1.0.0, b ^1.0.0");
    registry.add(two, "foo", "1.1.0", "x ^1.0.0, y ^1.0.0");
    registry.add(two, "a", "1.0.0", "b ^2.0.0");
    registry.add(two, "b", "1.0.0", "");
    registry.add(two, "b", "2.0.0", "");
    registry.add(two, "x", "1.0.0", "y ^2.0.0");
    registry.add(two, "y", "1.0.0", "");
    registry.add(two, "y", "2.0.0", "");
    run("branching", two);

    const three = registry.registry();
    registry.add(three, "root", "1.0.0", "missing ^1.0.0");
    run("nothing there", three);

    const four = registry.registry();
    registry.add(four, "root", "1.0.0", "foo ^2.0.0");
    registry.add(four, "foo", "1.0.0", "");
    run("nothing in range", four);
    return 0;
}
