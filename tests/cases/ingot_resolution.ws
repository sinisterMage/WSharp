// What `ingot resolve` promises, stated as a test.
//
// `semver.ws` beside this covers the set algebra -- what `^1.2.3` means and
// what a complement is. This covers the layer above it: given a universe of
// packages, which versions the solver chooses, and what it says when there is
// no choice to make. Those are the promises a lockfile rests on, so they are
// written down in `docs/resolution.md` and pinned here.
//
// The solver takes a `Provider`, which is two `fn` values, so a case can hand
// it a table in memory and it cannot tell the difference between that and the
// store. No network, no registry directory, no filesystem.
//
// expect: newest that satisfies: a=1.2.0
// expect: an exact pin is honoured: a=1.0.0
// expect: a pre-release is not a candidate unless asked for: a=1.2.0
// expect: asking for one by name makes it one: a=1.3.0-rc.1
// expect: a pre-release range does not admit a different package's: a=1.3.0-rc.1 b=1.0.0
// expect: a transitive bound narrows the choice: a=1.2.0 b=1.1.0
// expect: the solver backtracks off a version whose needs cannot be met: a=1.1.0 b=1.0.0
// expect: a package nothing supplies:
// expect: Because no versions of ghost match >=1.0.0 <2.0.0 and root 0.0.0 depends on ghost >=1.0.0 <2.0.0, version solving failed.
// expect: a range no published version is in:
// expect: Because no versions of a match >=9.0.0 <10.0.0 and root 0.0.0 depends on a >=9.0.0 <10.0.0, version solving failed.
// expect: two requirements that cannot both hold:
// expect: Because root 0.0.0 depends on a >=1.0.0 <1.1.0 and root 0.0.0 depends on a >=1.2.0 <1.3.0, version solving failed.
const list = @import("std/list");
const pubgrub = @import("ingot/pubgrub");
const semver = @import("ingot/semver");
const text = @import("std/str");

// ---------------------------------------------------------------------------
// A universe in memory
// ---------------------------------------------------------------------------

/// One published release: a name, a version, and what it depends on.
const Release = struct { name: str, version: semver.Version, needs: list.List[pubgrub.Need] };

/// `name` at `spec`, depending on nothing.
fn release(name: str, spec: str) Release {
    var none: list.List[pubgrub.Need] = list.new();
    return Release{ .name = name, .version = at(spec), .needs = none };
}

/// `name` at `spec`, depending on `on` at `req`.
fn release_on(name: str, spec: str, on: str, req: str) Release {
    var needs: list.List[pubgrub.Need] = list.new();
    list.push(needs, want(on, req));
    return Release{ .name = name, .version = at(spec), .needs = needs };
}

/// A version, or 0.0.0 if the case wrote one this library cannot read -- which
/// `semver.ws` is the place to catch, so here it must not become a second
/// spelling of the same test.
fn at(spec: str) semver.Version {
    return semver.parse(spec) orelse semver.zero();
}

/// One requirement: `name` satisfying `req`.
fn want(name: str, req: str) pubgrub.Need {
    const r = semver.requirement(req) orelse semver.any();
    return pubgrub.Need{ .package = name, .range = r };
}

/// A provider over `world`, with `root` answering for itself.
///
/// The root package is a release like any other, so its own requirements are
/// terms in the search rather than a special case -- which is how `plan.ws`
/// builds the real one.
fn over(world: list.List[Release]) pubgrub.Provider {
    return pubgrub.Provider{
        .versions = fn (p: str) list.List[semver.Version] {
            var out: list.List[semver.Version] = list.new();
            const all = list.to_array(world);
            const n = array_len(all);
            var i = 0;
            while (i < n) : (i += 1) {
                if (text.eq(all[i].name, p)) { list.push(out, all[i].version); }
            }
            return out;
        },
        .dependencies = fn (p: str, v: semver.Version) list.List[pubgrub.Need] {
            const all = list.to_array(world);
            const n = array_len(all);
            var i = 0;
            while (i < n) : (i += 1) {
                if (text.eq(all[i].name, p) and semver.eq(all[i].version, v)) { return all[i].needs; }
            }
            var none: list.List[pubgrub.Need] = list.new();
            return none;
        },
    };
}

/// `array.len` behind a name, so the loops above read the same as the rest.
///
/// A builtin call is a stack walk under `--gc-stress`, so every loop here
/// takes the length once and then calls nothing.
fn array_len(a: []Release) i64 {
    var n = 0;
    for (a) |_| { n += 1; }
    return n;
}

// ---------------------------------------------------------------------------
// Asking
// ---------------------------------------------------------------------------

/// Solve, and report the chosen version of each name in `report_on`.
fn choose(label: str, world: list.List[Release], root_needs: list.List[pubgrub.Need],
          report_on: []str) void {
    var w: list.List[Release] = list.new();
    const existing = list.to_array(world);
    const m = array_len(existing);
    var k = 0;
    while (k < m) : (k += 1) { list.push(w, existing[k]); }
    list.push(w, Release{ .name = "root", .version = semver.zero(), .needs = root_needs });

    const answer = pubgrub.solve(over(w), "root", semver.zero());
    if (!answer.ok) {
        print(text.concat(label, ":"));
        print(text.trim(answer.report));
        return;
    }

    var line = text.concat(label, ":");
    for (report_on) |name| {
        line = text.concat(line, text.concat(" ", text.concat(name,
            text.concat("=", picked(answer, name)))));
    }
    print(line);
    return;
}

/// What `name` was chosen at, or `-` if the solution does not name it.
fn picked(a: pubgrub.Answer, name: str) str {
    const n = list.len(a.names);
    var i = 0;
    while (i < n) : (i += 1) {
        if (text.eq(list.get(a.names, i), name)) { return semver.render(list.get(a.versions, i)); }
    }
    return "-";
}

/// A root that wants one thing.
fn needing(name: str, req: str) list.List[pubgrub.Need] {
    var needs: list.List[pubgrub.Need] = list.new();
    list.push(needs, want(name, req));
    return needs;
}

fn main() i64 {
    // The newest version inside the range, not the oldest and not the newest
    // published. This is the whole of the choice rule.
    var plain: list.List[Release] = list.new();
    list.push(plain, release("a", "1.0.0"));
    list.push(plain, release("a", "1.1.0"));
    list.push(plain, release("a", "1.2.0"));
    list.push(plain, release("a", "2.0.0"));
    choose("newest that satisfies", plain, needing("a", "^1.0.0"), []str{ "a" });

    // `=1.0.0` is a pin: one version, and the solver may not walk off it even
    // though newer ones are right there.
    choose("an exact pin is honoured", plain, needing("a", "=1.0.0"), []str{ "a" });

    // A pre-release is below the release it leads to and inside the same
    // caret range, and is still not chosen -- the policy sits on top of the
    // algebra rather than inside it.
    var pre: list.List[Release] = list.new();
    list.push(pre, release("a", "1.0.0"));
    list.push(pre, release("a", "1.2.0"));
    list.push(pre, release("a", "1.3.0-rc.1"));
    choose("a pre-release is not a candidate unless asked for", pre,
        needing("a", "^1.0.0"), []str{ "a" });

    // Naming one in the requirement is how it is asked for.
    choose("asking for one by name makes it one", pre,
        needing("a", ">=1.3.0-rc.1"), []str{ "a" });

    // And asking for one of `a` says nothing about `b`: the policy is per
    // package, read off the range that package is being chosen under.
    var mixed: list.List[Release] = list.new();
    list.push(mixed, release("a", "1.3.0-rc.1"));
    list.push(mixed, release("b", "1.0.0"));
    list.push(mixed, release("b", "1.1.0-rc.1"));
    var both: list.List[pubgrub.Need] = list.new();
    list.push(both, want("a", ">=1.3.0-rc.1"));
    list.push(both, want("b", "^1.0.0"));
    choose("a pre-release range does not admit a different package's", mixed, both,
        []str{ "a", "b" });

    // A dependency's own requirement is a constraint on the whole solution:
    // `a 1.2.0` needs `b <1.2.0`, so `b 1.2.0` is out even though the root
    // would otherwise take it.
    var chain: list.List[Release] = list.new();
    list.push(chain, release_on("a", "1.2.0", "b", ">=1.0.0, <1.2.0"));
    list.push(chain, release("b", "1.0.0"));
    list.push(chain, release("b", "1.1.0"));
    list.push(chain, release("b", "1.2.0"));
    var chain_needs: list.List[pubgrub.Need] = list.new();
    list.push(chain_needs, want("a", "^1.0.0"));
    list.push(chain_needs, want("b", "^1.0.0"));
    choose("a transitive bound narrows the choice", chain, chain_needs, []str{ "a", "b" });

    // The newest `a` asks for a `b` nothing supplies, so the solver takes the
    // one below it rather than failing. This is the step a resolver without
    // backtracking gets wrong, and it gets it wrong by reporting a conflict
    // that a different choice would have avoided.
    var back: list.List[Release] = list.new();
    list.push(back, release_on("a", "1.2.0", "b", "^3.0.0"));
    list.push(back, release_on("a", "1.1.0", "b", "^1.0.0"));
    list.push(back, release("b", "1.0.0"));
    choose("the solver backtracks off a version whose needs cannot be met", back,
        needing("a", "^1.0.0"), []str{ "a", "b" });

    // Three ways to have no answer, each with the sentence it prints. These
    // are the contract `docs/resolution.md` states, and the reason they are
    // here is that a resolver's failure message is as much its interface as
    // its success.
    choose("a package nothing supplies", plain, needing("ghost", "^1.0.0"), []str{ });
    choose("a range no published version is in", plain, needing("a", "^9.0.0"), []str{ });

    var contradiction: list.List[pubgrub.Need] = list.new();
    list.push(contradiction, want("a", "~1.0.0"));
    list.push(contradiction, want("a", "~1.2.0"));
    choose("two requirements that cannot both hold", plain, contradiction, []str{ });

    return 0;
}
