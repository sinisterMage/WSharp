// PubGrub: version solving that can say *why*.
//
// The thing this item was actually built around. A resolver that answers
// "unsatisfiable" has told you nothing you can act on; one that answers
//
//     Because myapp depends on foo ^1.0.0 and foo 1.0.0 depends on
//     bar ^2.0.0, myapp requires bar ^2.0.0.
//     And because no versions of bar match >=2.0.0 <3.0.0, version solving
//     failed.
//
// has told you which line of which manifest to change. Getting that out is not
// a layer over the search -- it *is* the search, recorded: every incompatibility
// the solver derives keeps the two it was derived from, and the report is a
// walk of that graph.
//
// The algorithm is Natalie Weizenbaum's, as written up for Dart's `pub`. The
// shape it takes here is:
//
//   * A **term** is "this package's version is in this set", or is not.
//   * An **incompatibility** is a set of terms that cannot all hold at once.
//     `{foo 1.0.0, not bar ^2.0.0}` says foo 1.0.0 depends on bar ^2.0.0.
//   * The **partial solution** is an ordered list of assignments, each either a
//     decision or something derived from an incompatibility.
//   * **Unit propagation** derives what must follow; **conflict resolution**
//     turns a contradiction into a new incompatibility and backjumps to where
//     it would have been noticed.
//
// Every set operation is `ingot/semver`'s, which is why that module represents
// a range as intervals: unit propagation takes complements constantly.
const array = @import("std/array");
const list = @import("std/list");
const semver = @import("ingot/semver");
const text = @import("std/str");

// ---------------------------------------------------------------------------
// Terms
// ---------------------------------------------------------------------------

/// "`package`'s version is in `range`", or -- when `positive` is false -- that
/// it is not.
///
/// The two are not interchangeable even though one is the other's complement:
/// a *positive* term is what makes the solver go and look for a version, and a
/// negative one is only a constraint on a package something else may select.
pub const Term = struct { package: str, range: semver.Range, positive: bool };

pub fn term(package: str, range: semver.Range) Term {
    return Term{ .package = package, .range = range, .positive = true };
}

pub fn not_term(package: str, range: semver.Range) Term {
    return Term{ .package = package, .range = range, .positive = false };
}

pub fn negate(t: Term) Term {
    return Term{ .package = t.package, .range = t.range, .positive = !t.positive };
}

/// The versions a term allows, for a term that names some.
///
/// Only meaningful for a positive term. A *negative* one is satisfied by the
/// package being absent altogether, and no set of versions says that -- which
/// is the whole reason `relates` below has four cases rather than one.
pub fn allows(t: Term) semver.Range {
    if (t.positive) { return t.range; }
    return semver.complement(t.range);
}

/// Both at once.
///
/// The set is the intersection of what each allows, which is the whole of the
/// arithmetic. What is *not* free is the polarity: a term stays positive if
/// either side was, because "something requires this package" survives being
/// narrowed by a constraint, and two constraints on a package nothing has asked
/// for still do not ask for it. Getting that backwards makes the solver go
/// looking for a version of a package no one wants.
pub fn term_intersect(a: Term, b: Term) Term {
    if (a.positive and b.positive) {
        return Term{ .package = a.package, .range = semver.intersect(a.range, b.range), .positive = true };
    }
    if (!a.positive and !b.positive) {
        return Term{ .package = a.package, .range = semver.unite(a.range, b.range), .positive = false };
    }
    // One of each: what the positive one allows, less what the negative one
    // rules out. The result is positive, because "something requires this
    // package" survives being narrowed by a constraint.
    const yes = if (a.positive) a else b;
    const no = if (a.positive) b else a;
    return Term{ .package = a.package, .range = semver.without(yes.range, no.range), .positive = true };
}

pub fn term_difference(a: Term, b: Term) Term { return term_intersect(a, negate(b)); }

/// A term that says nothing, and can be left out.
pub fn term_empty(t: Term) bool { return semver.is_empty(t.range); }

/// How one term relates to another about the same package.
pub const SATISFIED = 0;
pub const CONTRADICTED = 1;
pub const INCONCLUSIVE = 2;

/// Whether `a` holding means `b` holds, means `b` cannot, or settles nothing.
///
/// Four cases rather than one piece of set arithmetic, and the case that makes
/// the difference is a negative `a` against a positive `b`: "this package is
/// not in A" never implies "it is in B", because it may not be selected at
/// all. Treating a negative term as its complement collapses that -- and
/// `not foo any-version`, which is what every dependency starts life as, then
/// looks like a term that can never hold.
pub fn relates(a: Term, b: Term) i64 {
    if (a.positive) {
        if (b.positive) {
            if (semver.subset(a.range, b.range)) { return SATISFIED; }
            if (semver.is_empty(semver.intersect(a.range, b.range))) { return CONTRADICTED; }
            return INCONCLUSIVE;
        }
        if (semver.is_empty(semver.intersect(a.range, b.range))) { return SATISFIED; }
        if (semver.subset(a.range, b.range)) { return CONTRADICTED; }
        return INCONCLUSIVE;
    }
    if (b.positive) {
        if (semver.subset(b.range, a.range)) { return CONTRADICTED; }
        return INCONCLUSIVE;
    }
    if (semver.subset(b.range, a.range)) { return SATISFIED; }
    return INCONCLUSIVE;
}

/// An incompatibility is a conjunction, so two terms about one package are one
/// term about it.
///
/// Not a tidying step: resolution routinely produces a pair like
/// `{not foo ^1.0.0, foo 2.0.0}`, which merged is `{foo 2.0.0}` -- the clause
/// that actually rules something out, and the one `terminal` can recognise.
/// Left unmerged, the two are asked about the same package independently, and
/// the search learns nothing and runs for ever.
fn merge(terms: list.List[Term]) list.List[Term] {
    var out: list.List[Term] = list.new();
    for (list.to_array(terms)) |t| {
        var at = -1;
        const n = list.len(out);
        var i = 0;
        while (i < n) : (i += 1) {
            if (text.eq(list.get(out, i).package, t.package)) { at = i; break; }
        }
        if (at >= 0) {
            list.set(out, at, term_intersect(list.get(out, at), t));
        } else {
            list.push(out, t);
        }
    }
    return out;
}

// ---------------------------------------------------------------------------
// Incompatibilities, and where they came from
// ---------------------------------------------------------------------------

/// Why an incompatibility is true. A lattice, because the report is a walk of
/// it and each kind reads differently.
pub const Cause = struct { };
/// "the root package must be selected".
pub const RootCause = struct : Cause { };
/// "this package has no version in this range".
pub const NoVersions = struct : Cause { };
/// "this version of this package depends on that".
pub const Dependency = struct : Cause { };
/// Derived by resolution from two others. The pair *is* the explanation.
pub const Conflict = struct : Cause { left: Incompatibility, right: Incompatibility };

pub const Incompatibility = struct {
    terms: list.List[Term],
    cause: Cause,
    /// Identity, so that the report can number one that is used twice rather
    /// than write it out twice.
    id: i64,
};

fn is_derived(i: Incompatibility) bool { return derived_cause(i.cause); }
fn derived_cause(c: Cause) bool { return false; }
fn derived_cause(c: Conflict) bool { return true; }

fn conflict_parts(i: Incompatibility) ?Conflict { return parts_of(i.cause); }
fn parts_of(c: Cause) ?Conflict { return null; }
fn parts_of(c: Conflict) ?Conflict { return c; }

/// An incompatibility with nothing in it, or with only the root package left,
/// is where conflict resolution stops: there is nothing further to blame.
fn terminal(i: Incompatibility, root: str) bool {
    const n = list.len(i.terms);
    if (n == 0) { return true; }
    if (n != 1) { return false; }
    const t = list.get(i.terms, 0);
    return t.positive and text.eq(t.package, root);
}

// ---------------------------------------------------------------------------
// What the solver is told about the world
// ---------------------------------------------------------------------------

/// One thing a version depends on.
pub const Need = struct { package: str, range: semver.Range };

/// Where versions and dependencies come from.
///
/// A struct of `fn` values, which is what a language with multiple dispatch and
/// no interfaces has: `std/hash.Hash` is the same shape. It lets a test hand
/// the solver a table in memory and the tool hand it the store, with the solver
/// unable to tell the difference.
///
/// Neither can fail. A provider that cannot answer says "no versions", which
/// the solver already has to explain well, and an error union here would have
/// to be threaded through every step of the search for a case that reads the
/// same at the end.
pub const Provider = struct {
    versions: fn(str) list.List[semver.Version],
    dependencies: fn(str, semver.Version) list.List[Need],
};

// ---------------------------------------------------------------------------
// The partial solution
// ---------------------------------------------------------------------------

const DECISION = 0;
const DERIVATION = 1;

const Assignment = struct {
    t: Term,
    level: i64,
    kind: i64,
    /// What it was derived from; null for a decision.
    cause: ?Incompatibility,
};

const State = struct {
    provider: Provider,
    root: str,
    incompatibilities: list.List[Incompatibility],
    assignments: list.List[Assignment],
    level: i64,
    next_id: i64,
    /// What conflict resolution ran out of things to blame on -- the failure
    /// itself, and the root of the graph the report walks.
    failure: ?Incompatibility,
    /// Bounded so that a provider that keeps answering can never hang the tool.
    steps: i64,
};

/// The accumulated term for one package, or null if nothing has been assigned.
fn derived(s: State, package: str) ?Term {
    var found: ?Term = null;
    for (list.to_array(s.assignments)) |a| {
        if (!text.eq(a.t.package, package)) { continue; }
        if (found) |so_far| { found = term_intersect(so_far, a.t); } else { found = a.t; }
    }
    return found;
}

/// How the partial solution relates to one term.
fn relation(s: State, t: Term) i64 {
    // A positive term over no versions can never hold, whatever has been
    // assigned. Merging can produce one, and without this the solver would
    // treat it as the loose end it is one step from tying.
    if (t.positive and semver.is_empty(t.range)) { return CONTRADICTED; }
    const d = derived(s, t.package) orelse return INCONCLUSIVE;
    return relates(d, t);
}

fn decided(s: State, package: str) bool {
    for (list.to_array(s.assignments)) |a| {
        if (a.kind == DECISION and text.eq(a.t.package, package)) { return true; }
    }
    return false;
}

/// The index of the first assignment at which `t` becomes satisfied.
///
/// -1 when nothing does, which conflict resolution treats as "cannot happen":
/// it is only ever asked about terms the solution already satisfies.
fn satisfier(s: State, t: Term) i64 {
    var so_far: ?Term = null;
    const n = list.len(s.assignments);
    var i = 0;
    while (i < n) : (i += 1) {
        const a = list.get(s.assignments, i);
        if (!text.eq(a.t.package, t.package)) { continue; }
        if (so_far) |acc| { so_far = term_intersect(acc, a.t); } else { so_far = a.t; }
        if (so_far) |acc| {
            if (relates(acc, t) == SATISFIED) { return i; }
        }
    }
    return -1;
}

fn derive(s: State, t: Term, cause: Incompatibility) void {
    list.push(s.assignments, Assignment{
        .t = t,
        .level = s.level,
        .kind = DERIVATION,
        .cause = cause,
    });
    return;
}

fn decide(s: State, package: str, v: semver.Version) void {
    s.level += 1;
    list.push(s.assignments, Assignment{
        .t = term(package, semver.exactly(v)),
        .level = s.level,
        .kind = DECISION,
        .cause = null,
    });
    return;
}

/// Drop everything above `level`. The backjump: not one step, but all the way
/// to where the conflict would first have been noticed.
fn backtrack(s: State, level: i64) void {
    var kept: list.List[Assignment] = list.new();
    for (list.to_array(s.assignments)) |a| {
        if (a.level <= level) { list.push(kept, a); }
    }
    s.assignments = kept;
    s.level = level;
    return;
}

fn add_incompatibility(s: State, terms: list.List[Term], cause: Cause) Incompatibility {
    const i = Incompatibility{ .terms = merge(terms), .cause = cause, .id = s.next_id };
    s.next_id += 1;
    list.push(s.incompatibilities, i);
    return i;
}

// ---------------------------------------------------------------------------
// Solving
// ---------------------------------------------------------------------------

/// What `solve` answers with.
///
/// A list rather than a map, for `std`'s reason: there is no map, and a
/// solution has tens of entries. `report` is the whole of the failure and is
/// meant to be printed as it stands.
pub const Answer = struct {
    ok: bool,
    names: list.List[str],
    versions: list.List[semver.Version],
    report: str,
};

/// The most steps the search will take before giving up.
///
/// PubGrub terminates, but a *provider* need not: one that answers with a new
/// version every time it is asked would search for ever. A tool that hangs is
/// worse than one that says it gave up.
const MAX_STEPS = 20000;

pub fn solve(provider: Provider, root: str, root_version: semver.Version) Answer {
    var incompatibilities: list.List[Incompatibility] = list.new();
    var assignments: list.List[Assignment] = list.new();
    const s = State{
        .provider = provider,
        .root = root,
        .incompatibilities = incompatibilities,
        .assignments = assignments,
        .level = 0,
        .next_id = 0,
        .failure = null,
        .steps = 0,
    };

    // "It is not the case that the root package is not this version" -- which
    // is how the search is told to start from somewhere.
    var start: list.List[Term] = list.new();
    list.push(start, not_term(root, semver.exactly(root_version)));
    add_incompatibility(s, start, RootCause{});

    var next = root;
    var going = true;
    while (going) {
        const failed = propagate(s, next);
        if (failed) { return failure(s); }
        if (s.steps > MAX_STEPS) { return gave_up(); }
        const chosen = choose(s);
        if (chosen) |package| { next = package; } else { going = false; }
    }
    return solution(s);
}

/// Derive everything that follows, and answer with the incompatibility that
/// could not be resolved if there is one.
fn propagate(s: State, package: str) bool {
    var changed: list.List[str] = list.new();
    list.push(changed, package);
    while (list.len(changed) > 0) {
        s.steps += 1;
        if (s.steps > MAX_STEPS) { return true; }
        const name = list.pop(changed);
        // Backwards, so that what was learned most recently is tried first --
        // which is what makes the search notice a fresh conflict quickly.
        var i = list.len(s.incompatibilities) - 1;
        while (i >= 0) : (i -= 1) {
            const incompat = list.get(s.incompatibilities, i);
            if (!mentions(incompat, name)) { continue; }
            const outcome = check(s, incompat);
            if (outcome.state == CONTRADICTED) { continue; }
            if (outcome.state == SATISFIED) {
                const resolved = resolve_conflict(s, incompat) orelse return true;
                var again: list.List[str] = list.new();
                const t = unsatisfied_of(s, resolved);
                if (t) |only| {
                    derive(s, negate(only), resolved);
                    list.push(again, only.package);
                }
                changed = again;
                i = -1;
                continue;
            }
            if (outcome.term) |only| {
                derive(s, negate(only), incompat);
                if (!holds(changed, only.package)) { list.push(changed, only.package); }
            }
        }
    }
    return false;
}

/// Whether an incompatibility is satisfied, contradicted, or one term short of
/// satisfied -- and which term that is.
const Outcome = struct { state: i64, term: ?Term };

fn check(s: State, i: Incompatibility) Outcome {
    var unsatisfied: ?Term = null;
    for (list.to_array(i.terms)) |t| {
        const r = relation(s, t);
        if (r == CONTRADICTED) { return Outcome{ .state = CONTRADICTED, .term = null }; }
        if (r == INCONCLUSIVE) {
            // Two loose ends means nothing follows yet.
            if (unsatisfied) |already| { return Outcome{ .state = INCONCLUSIVE, .term = null }; }
            unsatisfied = t;
        }
    }
    if (unsatisfied) |only| { return Outcome{ .state = INCONCLUSIVE, .term = only }; }
    return Outcome{ .state = SATISFIED, .term = null };
}

fn unsatisfied_of(s: State, i: Incompatibility) ?Term {
    return check(s, i).term;
}

fn mentions(i: Incompatibility, package: str) bool {
    for (list.to_array(i.terms)) |t| {
        if (text.eq(t.package, package)) { return true; }
    }
    return false;
}

fn holds(l: list.List[str], want: str) bool {
    for (list.to_array(l)) |v| {
        if (text.eq(v, want)) { return true; }
    }
    return false;
}

/// Turn a satisfied incompatibility into one that is not, backjumping to where
/// it would have been noticed.
///
/// Null means the search has run out of things to blame: the conflict follows
/// from the root package alone, and there is no solution.
fn resolve_conflict(s: State, start: Incompatibility) ?Incompatibility {
    var incompat = start;
    var made_one = false;
    while (s.steps < MAX_STEPS) {
        s.steps += 1;
        if (terminal(incompat, s.root)) {
            // Nothing left to blame: this is the failure, and the graph behind
            // it is the report.
            s.failure = incompat;
            return null;
        }

        // The term whose satisfier came last, and the level of the latest of
        // the others -- which is where this conflict was really decided.
        var recent_term: ?Term = null;
        var recent_at = -1;
        var previous_level = 1;
        for (list.to_array(incompat.terms)) |t| {
            const at = satisfier(s, t);
            if (at < 0) { continue; }
            if (at > recent_at) {
                if (recent_at >= 0) {
                    previous_level = larger(previous_level, list.get(s.assignments, recent_at).level);
                }
                recent_term = t;
                recent_at = at;
            } else {
                previous_level = larger(previous_level, list.get(s.assignments, at).level);
            }
        }
        const only = recent_term orelse {
            s.failure = incompat;
            return null;
        };
        const satisfying = list.get(s.assignments, recent_at);

        // Either the conflict predates the most recent decision, or that
        // decision is what caused it: both mean this incompatibility is the
        // one to keep, once the solution has been rewound to before it held.
        if (previous_level < satisfying.level or satisfying.kind == DECISION) {
            backtrack(s, previous_level);
            if (made_one) { list.push(s.incompatibilities, incompat); }
            return incompat;
        }

        // Resolution: everything both say, minus the package they disagree
        // about, plus whatever of the term the satisfier did not cover.
        const cause = satisfying.cause orelse {
            s.failure = incompat;
            return null;
        };
        var terms: list.List[Term] = list.new();
        for (list.to_array(incompat.terms)) |t| {
            if (!text.eq(t.package, only.package)) { list.push(terms, t); }
        }
        for (list.to_array(cause.terms)) |t| {
            if (!text.eq(t.package, satisfying.t.package)) { list.push(terms, t); }
        }
        // What the satisfier allowed that the term did not: the satisfier is
        // why the term held, so anything it allowed *beyond* the term is
        // still open and has to be carried into the new incompatibility. The
        // subtraction is that way round and not the other; reversed, the
        // search learns a clause that rules nothing out and never terminates.
        const left_over = term_difference(satisfying.t, only);
        if (!term_empty(left_over)) { list.push(terms, negate(left_over)); }

        incompat = Incompatibility{
            .terms = merge(terms),
            .cause = Conflict{ .left = incompat, .right = cause },
            .id = s.next_id,
        };
        s.next_id += 1;
        made_one = true;
    }
    return null;
}

fn larger(a: i64, b: i64) i64 { if (a > b) { return a; } return b; }

/// Pick a package to decide, and decide it. Null means everything positive has
/// been decided, which is the answer.
fn choose(s: State) ?str {
    var package = "";
    var want: ?Term = null;
    for (list.to_array(s.assignments)) |a| {
        if (!a.t.positive) { continue; }
        if (decided(s, a.t.package)) { continue; }
        const d = derived(s, a.t.package) orelse continue;
        if (!d.positive) { continue; }
        package = a.t.package;
        want = d;
        break;
    }
    const wanted = want orelse return null;
    const allowed = wanted.range;

    const versions = (s.provider.versions)(package);
    var best: ?semver.Version = null;
    for (list.to_array(versions)) |v| {
        if (!semver.contains(allowed, v)) { continue; }
        // A pre-release is not a candidate unless it was asked for. The set
        // algebra stays honest about `1.0.0-rc.1 < 1.0.0`; this is the policy
        // on top of it, and it belongs here rather than in the algebra.
        if (text.len(v.pre) > 0 and !semver.mentions_prerelease(allowed)) { continue; }
        if (best) |so_far| {
            if (semver.less(so_far, v)) { best = v; }
        } else {
            best = v;
        }
    }

    const version = best orelse {
        var terms: list.List[Term] = list.new();
        list.push(terms, term(package, allowed));
        add_incompatibility(s, terms, NoVersions{});
        return package;
    };

    // Its dependencies become incompatibilities: "this version, and not that
    // requirement" cannot both hold.
    var conflicting = false;
    for (list.to_array((s.provider.dependencies)(package, version))) |need| {
        var terms: list.List[Term] = list.new();
        list.push(terms, term(package, semver.exactly(version)));
        list.push(terms, not_term(need.package, need.range));
        const incompat = add_incompatibility(s, terms, Dependency{});
        // If everything except the term about this package already holds, then
        // deciding would conflict at once. Leave it undecided and let unit
        // propagation say so.
        if (satisfied_apart_from(s, incompat, package)) { conflicting = true; }
    }
    if (!conflicting) { decide(s, package, version); }
    return package;
}

fn satisfied_apart_from(s: State, i: Incompatibility, package: str) bool {
    for (list.to_array(i.terms)) |t| {
        if (text.eq(t.package, package)) { continue; }
        if (relation(s, t) != SATISFIED) { return false; }
    }
    return true;
}

fn solution(s: State) Answer {
    var names: list.List[str] = list.new();
    var versions: list.List[semver.Version] = list.new();
    for (list.to_array(s.assignments)) |a| {
        if (a.kind != DECISION) { continue; }
        if (text.eq(a.t.package, s.root)) { continue; }
        list.push(names, a.t.package);
        list.push(versions, chosen(a.t));
    }
    return Answer{ .ok = true, .names = names, .versions = versions, .report = "" };
}

/// The one version a decision's term allows.
fn chosen(t: Term) semver.Version {
    for (list.to_array(t.range.parts)) |i| { return i.lo.v; }
    return semver.zero();
}

fn failure(s: State) Answer {
    var names: list.List[str] = list.new();
    var versions: list.List[semver.Version] = list.new();
    const bad = s.failure orelse return gave_up();
    return Answer{ .ok = false, .names = names, .versions = versions, .report = explain(bad) };
}

fn gave_up() Answer {
    var names: list.List[str] = list.new();
    var versions: list.List[semver.Version] = list.new();
    return Answer{
        .ok = false,
        .names = names,
        .versions = versions,
        .report = "gave up: the search took too many steps, which means a provider kept answering with new versions",
    };
}

// ---------------------------------------------------------------------------
// The report
// ---------------------------------------------------------------------------
//
// The feature. Every derived incompatibility keeps the two it came from, so
// the failure is a graph and the report is a walk of it: each derivation
// becomes one line saying what it followed from, and a derivation used more
// than once is written out once and referred to by number afterwards.
//
// The walk is post-order, so a line only ever refers to a line above it.

/// Turn the incompatibility the search failed on into something to read.
pub fn explain(bad: Incompatibility) str {
    var counts: list.List[i64] = list.new();
    var ids: list.List[i64] = list.new();
    count_uses(bad, ids, counts);

    var lines: list.List[str] = list.new();
    var numbered_ids: list.List[i64] = list.new();
    var numbers: list.List[i64] = list.new();
    write_out(bad, lines, numbered_ids, numbers, ids, counts, true);

    var out = "";
    const n = list.len(lines);
    var i = 0;
    while (i < n) : (i += 1) {
        if (i > 0) { out = text.concat(out, "\n"); }
        out = text.concat(out, list.get(lines, i));
    }
    return out;
}

/// How many times each derived incompatibility is reached.
///
/// One that is reached twice is the whole reason lines are numbered: writing
/// its derivation out again in full is how these reports become unreadable.
fn count_uses(i: Incompatibility, ids: list.List[i64], counts: list.List[i64]) void {
    const at = index_of(ids, i.id);
    if (at >= 0) {
        list.set(counts, at, list.get(counts, at) + 1);
        return;
    }
    list.push(ids, i.id);
    list.push(counts, 1);
    if (conflict_parts(i)) |pair| {
        count_uses(pair.left, ids, counts);
        count_uses(pair.right, ids, counts);
    }
    return;
}

fn index_of(ids: list.List[i64], id: i64) i64 {
    const n = list.len(ids);
    var i = 0;
    while (i < n) : (i += 1) {
        if (list.get(ids, i) == id) { return i; }
    }
    return -1;
}

/// Write the derivation of `i`, and answer with the line number it was given,
/// or 0 for a line that was not numbered.
fn write_out(
    i: Incompatibility,
    lines: list.List[str],
    numbered_ids: list.List[i64],
    numbers: list.List[i64],
    ids: list.List[i64],
    counts: list.List[i64],
    conclusion: bool,
) i64 {
    // Already written: refer to it rather than repeating it.
    const seen = index_of(numbered_ids, i.id);
    if (seen >= 0) { return list.get(numbers, seen); }

    const pair = conflict_parts(i) orelse return 0;

    const left = write_out(pair.left, lines, numbered_ids, numbers, ids, counts, false);
    const right = write_out(pair.right, lines, numbered_ids, numbers, ids, counts, false);

    var line = "Because ";
    line = text.concat(line, phrase(pair.left, left));
    line = text.concat(line, " and ");
    line = text.concat(line, phrase(pair.right, right));
    line = text.concat(line, ", ");
    if (conclusion) {
        line = text.concat(line, "version solving failed.");
    } else {
        line = text.concat(line, text.concat(describe(i), "."));
    }

    const at = index_of(ids, i.id);
    const shared = at >= 0 and list.get(counts, at) > 1;
    var number = 0;
    if (shared and !conclusion) {
        number = list.len(numbered_ids) + 1;
        list.push(numbered_ids, i.id);
        list.push(numbers, number);
        line = text.concat(text.concat("(", text.concat(text.from_int(number), ") ")), line);
    }
    list.push(lines, line);
    return number;
}

/// How one half of a derivation is named: its own words, or a reference to the
/// line that already explained it.
fn phrase(i: Incompatibility, number: i64) str {
    if (number > 0) {
        return text.concat(describe(i), text.concat(" (", text.concat(text.from_int(number), ")")));
    }
    return describe(i);
}

/// An incompatibility in words.
///
/// The shapes are worth spelling out separately: `{foo 1.0.0, not bar ^2.0.0}`
/// is a dependency and reads as one, and rendering it as "one of these must be
/// false" would be correct and useless.
pub fn describe(i: Incompatibility) str {
    const n = list.len(i.terms);
    if (n == 0) { return "version solving failed"; }
    if (n == 1) {
        const t = list.get(i.terms, 0);
        if (no_versions_cause(i.cause)) {
            return text.concat("no versions of ", text.concat(t.package,
                text.concat(" match ", semver.show(t.range))));
        }
        if (t.positive) {
            return text.concat(t.package, text.concat(" ", text.concat(semver.show(t.range),
                " cannot be used")));
        }
        return text.concat(t.package, text.concat(" ", text.concat(semver.show(t.range),
            " is required")));
    }
    if (n == 2) {
        const a = list.get(i.terms, 0);
        const b = list.get(i.terms, 1);
        if (a.positive and !b.positive) { return depends(a, b); }
        if (b.positive and !a.positive) { return depends(b, a); }
        if (a.positive and b.positive) {
            return text.concat(named(a), text.concat(" is incompatible with ", named(b)));
        }
    }
    var out = "one of ";
    var first = true;
    for (list.to_array(i.terms)) |t| {
        if (!first) { out = text.concat(out, ", "); }
        first = false;
        out = text.concat(out, named(t));
    }
    return text.concat(out, " must be left out");
}

fn depends(from: Term, on: Term) str {
    return text.concat(named(from), text.concat(" depends on ", named(on)));
}

fn named(t: Term) str {
    return text.concat(t.package, text.concat(" ", semver.show(t.range)));
}

fn no_versions_cause(c: Cause) bool { return false; }
fn no_versions_cause(c: NoVersions) bool { return true; }
