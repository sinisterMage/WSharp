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
const git = @import("ingot/git");
const list = @import("std/list");
const manifest = @import("ingot/manifest");
const path = @import("std/path");
const pubgrub = @import("ingot/pubgrub");
const semver = @import("ingot/semver");
const store = @import("ingot/store");
const tls = @import("std/tls");
const x509 = @import("std/x509");
const text = @import("std/str");

/// One package the search may choose, and where its files are.
pub const Source = struct {
    name: str,
    version: semver.Version,
    /// The directory it was read from: relative to the project for a path
    /// dependency, and inside the store for a fetched one.
    dir: str,
    /// How the lockfile names where it came from.
    origin: str,
    needs: list.List[pubgrub.Need],
    /// The same, by name, for the lockfile.
    deps: []str,
};

/// What a fetch needs to reach a remote, gathered once.
///
/// The trust store is the expensive half of an HTTPS connection and is parsed
/// once here rather than per request, which is what `http.request_with` exists
/// for. `ready` is false when there is no store to be had, and every git
/// dependency then fails with that rather than with a handshake error.
pub const Network = struct { cfg: tls.Config, ready: bool, tried: bool };

/// A network nothing has needed yet.
///
/// Reading the machine's certificate store costs about as much as a handshake,
/// so it is not done until a git dependency actually asks -- which means a
/// project of path dependencies resolves without touching it, and a machine
/// with no store can still use one.
pub fn network() Network {
    return Network{ .cfg = tls.client_config(""), .ready = false, .tried = false };
}

/// A network that will refuse, for a caller that must not reach one.
pub fn offline() Network {
    return Network{ .cfg = tls.client_config(""), .ready = false, .tried = true };
}

fn connect(f: fault.Fault, net: Network) bool {
    if (net.tried) { return net.ready; }
    net.tried = true;
    const roots = x509.system_roots() catch {
        fault.fail(f, "this machine has no certificate store, so nothing can be fetched");
        return false;
    };
    net.cfg = tls.roots_config("", roots);
    net.ready = true;
    return true;
}

/// Every package reachable from `m` by following path dependencies.
///
/// Breadth first, and a package reached twice by two routes is read once: two
/// spellings of one directory are one package, and `path.normalise` is what
/// makes them the same string.
pub fn discover(f: fault.Fault, net: Network, m: manifest.Manifest) ?list.List[Source] {
    var found: list.List[Source] = list.new();
    var dirs: list.List[str] = list.new();
    var queue: list.List[str] = list.new();
    var wanted: list.List[str] = list.new();
    var origins: list.List[str] = list.new();
    if (!enqueue(f, net, m, m.dir, queue, wanted, origins)) { return null; }
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
        const source = describe(f, sub, dir, list.get(origins, at)) orelse return null;
        list.push(found, source);
        if (!enqueue(f, net, sub, dir, queue, wanted, origins)) { return null; }
    }
    return found;
}

/// Add a manifest's path dependencies to the walk.
///
/// A dependency written as a *version* is left alone here: something else in
/// the graph may be the package it names, and this is not the place to decide
/// -- `unsourced` asks afterwards, when the whole graph is known.
fn enqueue(f: fault.Fault, net: Network, m: manifest.Manifest, from: str,
           queue: list.List[str], wanted: list.List[str], origins: list.List[str]) bool {
    for (list.to_array(m.deps)) |d| {
        if (text.len(d.git) > 0) {
            const dir = fetched(f, net, d) orelse return false;
            if (!holds(queue, dir)) {
                list.push(queue, dir);
                list.push(wanted, d.name);
                list.push(origins, source_name(d));
            }
            continue;
        }
        if (text.len(d.dir) == 0) { continue; }
        const dir = path.normalise(path.join(from, d.dir));
        if (!fs.is_dir(dir)) {
            fault.fail(f, text.concat(text.concat(text.concat("`", d.name), "` points at "),
                text.concat(dir, ", which is not a directory")));
            return false;
        }
        if (!holds(queue, dir)) {
            list.push(queue, dir);
            list.push(wanted, d.name);
            list.push(origins, text.concat("path+", dir));
        }
    }
    return true;
}

/// How a lockfile names a git dependency: the URL and the revision, which
/// together name one tree for ever.
fn source_name(d: manifest.Dep) str {
    return text.concat("git+", text.concat(d.git, text.concat("#", d.rev)));
}

/// A git dependency's files, in the store, and where they are.
///
/// Fetching is what `resolve` has to do before it can even read the package's
/// manifest, so it happens here rather than in `install` -- and it is
/// remembered, because a revision names one tree for ever and the second
/// `resolve` of a project should touch no network at all.
fn fetched(f: fault.Fault, net: Network, d: manifest.Dep) ?str {
    const home = store.home() catch {
        fault.fail(f, "cannot work out where the store is");
        return null;
    };
    const source = source_name(d);
    if (store.remembered(home, source)) |digest| {
        if (store.check(home, digest) == store.READY) { return store.entry(home, digest); }
    }
    if (text.len(d.rev) != 40) {
        fault.fail(f, text.concat(text.concat("`", d.name),
            "` needs a `rev` that is a full object id"));
        return null;
    }
    if (!connect(f, net)) {
        if (f.ok) {
            fault.fail(f, text.concat(text.concat("`", d.name),
                "` has not been fetched, and nothing here can fetch it"));
        }
        return null;
    }
    return pull(f, net, home, d.git, d.rev, source);
}

/// Fetch one revision into the store, and answer with where it landed.
fn pull(f: fault.Fault, net: Network, home: str, url: str, rev: str, source: str) ?str {
    const remote = git.Remote{ .url = url, .cfg = net.cfg };
    const pack = git.fetch(f, remote, rev) orelse return null;
    const files = git.files(f, pack, rev) orelse return null;
    var carried: list.List[store.File] = list.new();
    for (list.to_array(files)) |file| {
        list.push(carried, store.File{ .path = file.path, .data = file.data });
    }
    const digest = store.install_files(f, home, carried);
    if (!f.ok) { return null; }
    store.remember(f, home, source, digest);
    if (!f.ok) { return null; }
    return store.entry(home, digest);
}

/// The directory a lockfile's `source` names, fetching it if it is a revision
/// that is not in the store yet.
///
/// What `install` uses: a lockfile may name a git package on a machine that
/// has never seen it, and the whole point of `install` is to make the store
/// satisfy the lockfile.
pub fn source_dir(f: fault.Fault, net: Network, source: str) ?str {
    if (text.starts_with(source, "path+")) {
        return text.substr(source, 5, text.len(source));
    }
    if (!text.starts_with(source, "git+")) {
        fault.fail(f, text.concat("ingot does not know the source ", source));
        return null;
    }
    const home = store.home() catch {
        fault.fail(f, "cannot work out where the store is");
        return null;
    };
    if (store.remembered(home, source)) |digest| {
        if (store.check(home, digest) == store.READY) { return store.entry(home, digest); }
    }
    const rest = text.substr(source, 4, text.len(source));
    const hash_at = last_hash(rest);
    if (hash_at < 0) {
        fault.fail(f, text.concat("this source names no revision: ", source));
        return null;
    }
    if (!connect(f, net)) { return null; }
    return pull(f, net, home, text.substr(rest, 0, hash_at),
                text.substr(rest, hash_at + 1, text.len(rest)), source);
}

/// The last `#`, which is what separates a URL from a revision -- a URL may
/// hold one of its own.
fn last_hash(s: str) i64 {
    var i = text.len(s) - 1;
    while (i >= 0) : (i -= 1) {
        if (text.byte_at(s, i) == 35) { return i; }
    }
    return -1;
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
fn describe(f: fault.Fault, m: manifest.Manifest, dir: str, origin: str) ?Source {
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
    return Source{
        .name = m.name,
        .version = version,
        .dir = dir,
        .origin = origin,
        .needs = needs,
        .deps = names,
    };
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
pub fn resolve(f: fault.Fault, net: Network, m: manifest.Manifest) Plan {
    const found = discover(f, net, m) orelse return failed("");
    const root = describe(f, m, m.dir, "root") orelse return failed("");

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
            .source = source.origin,
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
