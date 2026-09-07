// Turning a manifest into a lockfile.
//
// The graph walk and the solver, kept apart from the verbs: `ingot resolve` is
// fifteen lines over this, and this is what a test can drive without a
// directory and a process.
//
// **Every resolution goes through PubGrub, even one with nothing to choose.**
// A path dependency offers exactly one version, so the search has no decisions
// to make -- but a requirement on that package still has to be *checked*, and
// two packages that want incompatible versions of a third still have to be
// told apart from two that agree. Running the trivial case through the same
// solver is what makes the interesting case say something useful rather than
// something new.
const array = @import("std/array");
const fault = @import("ingot/fault");
const fs = @import("std/fs");
const list = @import("std/list");
const manifest = @import("ingot/manifest");
const path = @import("std/path");
const pubgrub = @import("ingot/pubgrub");
const semver = @import("ingot/semver");
const store = @import("ingot/store");
const text = @import("std/str");

/// One package the search may choose, and where its files are.
pub const Source = struct {
    name: str,
    version: semver.Version,
    /// The directory it was read from, relative to the project.
    dir: str,
    needs: list.List[pubgrub.Need],
    /// The same, by name, for the lockfile.
    deps: []str,
};

/// Every package reachable from `m` by following path dependencies.
///
/// Breadth first, and a package reached twice by two routes is read once: two
/// spellings of one directory are one package, and `path.normalise` is what
/// makes them the same string.
pub fn discover(f: fault.Fault, m: manifest.Manifest) ?list.List[Source] {
    var found: list.List[Source] = list.new();
    var dirs: list.List[str] = list.new();
    var queue: list.List[str] = list.new();
    var wanted: list.List[str] = list.new();
    if (!enqueue(f, m, m.dir, queue, wanted)) { return null; }
    var at = 0;
    while (at < list.len(queue)) : (at += 1) {
        const dir = list.get(queue, at);
        if (holds(dirs, dir)) { continue; }
        list.push(dirs, dir);
        const sub = manifest.read(f, path.join(dir, manifest.MANIFEST_NAME)) orelse return null;
        // The directory has to hold the package that was asked for. Without
        // this the solver would answer "no versions of X match any version",
        // which is true and says nothing about the typed path that caused it.
        const asked = list.get(wanted, at);
        if (!text.eq(sub.name, asked)) {
            fault.fail(f, text.concat(text.concat(text.concat("`", asked), "` points at "),
                text.concat(dir, text.concat(", whose package is called ", sub.name))));
            return null;
        }
        const source = describe(f, sub, dir) orelse return null;
        list.push(found, source);
        if (!enqueue(f, sub, dir, queue, wanted)) { return null; }
    }
    return found;
}

/// Add a manifest's path dependencies to the walk.
///
/// A dependency written as a *version* is left alone here: something else in
/// the graph may be the package it names, and this is not the place to decide
/// -- `unsourced` asks afterwards, when the whole graph is known.
fn enqueue(f: fault.Fault, m: manifest.Manifest, from: str,
           queue: list.List[str], wanted: list.List[str]) bool {
    for (list.to_array(m.deps)) |d| {
        if (text.len(d.dir) == 0) { continue; }
        const dir = path.normalise(path.join(from, d.dir));
        if (!fs.is_dir(dir)) {
            fault.fail(f, text.concat(text.concat(text.concat("`", d.name), "` points at "),
                text.concat(dir, ", which is not a directory")));
            return false;
        }
        if (!holds(queue, dir)) { list.push(queue, dir); list.push(wanted, d.name); }
    }
    return true;
}

/// The first package something asks for that nothing in the graph supplies.
///
/// Saying "ingot cannot yet fetch one" beats the solver's own honest "no
/// versions of X match ^1.0.0": both are true, and only one names the thing to
/// do about it.
fn unsourced(root: Source, found: list.List[Source]) ?str {
    for (list.to_array(root.needs)) |need| {
        if (!supplied(root, found, need.package)) { return need.package; }
    }
    for (list.to_array(found)) |s| {
        for (list.to_array(s.needs)) |need| {
            if (!supplied(root, found, need.package)) { return need.package; }
        }
    }
    return null;
}

fn supplied(root: Source, found: list.List[Source], name: str) bool {
    if (text.eq(root.name, name)) { return true; }
    for (list.to_array(found)) |s| {
        if (text.eq(s.name, name)) { return true; }
    }
    return false;
}

/// A manifest as something the solver can choose.
fn describe(f: fault.Fault, m: manifest.Manifest, dir: str) ?Source {
    const version = semver.parse(m.version) orelse {
        fault.fail_at(f, path.join(dir, manifest.MANIFEST_NAME),
            text.concat(text.concat("`", m.version), "` is not a version"));
        return null;
    };
    var needs: list.List[pubgrub.Need] = list.new();
    var names = []str{};
    for (list.to_array(m.deps)) |d| {
        const range = wanted(f, m, d) orelse return null;
        list.push(needs, pubgrub.Need{ .package = d.name, .range = range });
        names = array.push(names, d.name);
    }
    return Source{ .name = m.name, .version = version, .dir = dir, .needs = needs, .deps = names };
}

/// What a dependency asks for.
///
/// A path dependency asks for nothing in particular -- the directory is the
/// answer, and there is exactly one version of it -- so it is `any`. A version
/// requirement is read as written, and a bad one is reported where it was
/// written rather than as "no versions match".
fn wanted(f: fault.Fault, m: manifest.Manifest, d: manifest.Dep) ?semver.Range {
    if (text.len(d.req) == 0) { return semver.any(); }
    const range = semver.requirement(d.req) orelse {
        fault.fail_at(f, m.name, text.concat(text.concat(text.concat("`", d.req),
            "` is not a version requirement, wanted by `"), text.concat(d.name, "`")));
        return null;
    };
    return range;
}

/// The versions and dependencies the solver asks about, out of what was found.
///
/// The root package answers for itself, so that its own requirements are terms
/// in the search like everything else's.
pub fn provider(root: Source, found: list.List[Source]) pubgrub.Provider {
    return pubgrub.Provider{
        .versions = fn (p: str) list.List[semver.Version] {
            var out: list.List[semver.Version] = list.new();
            if (text.eq(p, root.name)) { list.push(out, root.version); return out; }
            for (list.to_array(found)) |s| {
                if (text.eq(s.name, p)) { list.push(out, s.version); }
            }
            return out;
        },
        .dependencies = fn (p: str, v: semver.Version) list.List[pubgrub.Need] {
            if (text.eq(p, root.name)) { return root.needs; }
            for (list.to_array(found)) |s| {
                if (text.eq(s.name, p) and semver.eq(s.version, v)) { return s.needs; }
            }
            var none: list.List[pubgrub.Need] = list.new();
            return none;
        },
    };
}

/// What `resolve` answers with: a lockfile, or the solver's report.
pub const Plan = struct { lock: ?manifest.Lock, report: str };

/// Choose versions for everything `m` needs, and hash what was chosen.
pub fn resolve(f: fault.Fault, m: manifest.Manifest) Plan {
    const found = discover(f, m) orelse return failed("");
    const root = describe(f, m, m.dir) orelse return failed("");

    if (unsourced(root, found)) |name| {
        fault.fail(f, text.concat(text.concat("`", name),
            "` is not a path dependency, and ingot cannot yet fetch one"));
        return failed("");
    }

    const answer = pubgrub.solve(provider(root, found), root.name, root.version);
    if (!answer.ok) { return failed(answer.report); }

    var packages: list.List[manifest.Locked] = list.new();
    const n = list.len(answer.names);
    var i = 0;
    while (i < n) : (i += 1) {
        const name = list.get(answer.names, i);
        const version = list.get(answer.versions, i);
        const source = chosen(found, name, version) orelse {
            fault.fail(f, text.concat(text.concat("`", name), "` was chosen and then could not be found"));
            return failed("");
        };
        const digest = store.tree_hash(f, source.dir);
        if (!f.ok) { return failed(""); }
        list.push(packages, manifest.Locked{
            .name = source.name,
            .version = semver.render(source.version),
            .source = text.concat("path+", source.dir),
            .tree = text.concat("sha256:", digest),
            .deps = source.deps,
        });
    }
    return Plan{
        .lock = manifest.Lock{
            .version = manifest.LOCK_VERSION,
            .manifest = m.digest,
            .packages = packages,
        },
        .report = "",
    };
}

fn failed(report: str) Plan { return Plan{ .lock = null, .report = report }; }

fn chosen(found: list.List[Source], name: str, version: semver.Version) ?Source {
    for (list.to_array(found)) |s| {
        if (text.eq(s.name, name) and semver.eq(s.version, version)) { return s; }
    }
    return null;
}

fn holds(l: list.List[str], want: str) bool {
    for (list.to_array(l)) |v| {
        if (text.eq(v, want)) { return true; }
    }
    return false;
}
