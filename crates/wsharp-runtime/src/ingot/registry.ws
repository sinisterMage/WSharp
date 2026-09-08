// The package index: which versions of a package exist, and where each one is.
//
// Modelled on Julia's General registry, for the reason that shape exists at
// all: a resolver has to know what versions there *are* before it can choose
// between them, and asking a git host that one package at a time is not a
// conversation a solver can hold. So the answer is published as data -- a
// directory per package, saying what its versions are and what each depends on
// -- and a client reads it the way it reads any other file.
//
// ```
// Registry.toml                          the name, and the format version
// packages/<owner>/<name>/package.toml   the name, and where to fetch it from
// packages/<owner>/<name>/versions.toml  one `[[version]]` per release
// ```
//
// **A registry is a directory.** Fetching one over git is only how the
// directory arrives; `INGOT_REGISTRY` naming one is used where it lies, with no
// network at all. That is what makes a private registry, an offline checkout
// and this project's own tests the same case as the public one -- and it is the
// only reason any of this can be tested here, since the git client speaks HTTP
// and no case in this suite may stand up a server.
//
// **A release records its tree hash**, which is the store's own key. So
// `resolve` reads the index and writes a lockfile having fetched no package at
// all, and the fetch `install` does afterwards is checked against a hash the
// registry committed to -- `install` already refuses a tree whose digest is not
// the one the lockfile named, so the commitment costs nothing to enforce and is
// the difference between trusting a host and trusting a hash.
//
// **A published version is never edited**, which is a rule the registry's own
// CI keeps and this file relies on: `store.remembered` memoises a source string
// to a tree for ever, so a version that changed underneath would be a stale
// tree nothing could clear. A mistake is a new version, and a withdrawal is
// `yanked`.
const array = @import("std/array");
const fault = @import("ingot/fault");
const fs = @import("std/fs");
const git = @import("ingot/git");
const io = @import("std/io");
const list = @import("std/list");
const manifest = @import("ingot/manifest");
const os = @import("std/os");
const path = @import("std/path");
const pubgrub = @import("ingot/pubgrub");
const semver = @import("ingot/semver");
const store = @import("ingot/store");
const text = @import("std/str");
const tls = @import("std/tls");
const toml = @import("std/toml");

pub const REGISTRY_NAME = "Registry.toml";
pub const PACKAGE_NAME = "package.toml";
pub const VERSIONS_NAME = "versions.toml";

/// The format this ingot reads. An index from a newer one is refused rather
/// than guessed at, exactly as `manifest.LOCK_VERSION` is.
pub const INDEX_VERSION = 1;

/// The registry, when nothing says otherwise.
pub const DEFAULT_URL = "https://github.com/sinisterMage/Foundry.git";

/// The branch an index is fetched from. A registry has no revision that would
/// never move -- being current is the whole of its job -- so it is named by a
/// branch, which is what `git.ref_id` is for.
pub const DEFAULT_BRANCH = "refs/heads/main";

/// What says where the registry is: a directory, or a URL.
pub const ENV_NAME = "INGOT_REGISTRY";

/// One published version of a package.
pub const Release = struct {
    version: semver.Version,
    /// The commit, as a full object id: what `git.fetch` wants.
    rev: str,
    /// `sha256:<hex>`, the store key the fetched tree must hash to.
    tree: str,
    /// Withdrawn: not offered to the solver, and still readable, because a
    /// lockfile that already names it must still install.
    yanked: bool,
    needs: list.List[pubgrub.Need],
    /// The same by name, for the lockfile.
    deps: []str,
};

pub const Package = struct { name: str, repo: str, releases: list.List[Release] };

pub const Index = struct { dir: str, name: str, version: i64 };

// ---------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------

/// Whether `name` is a name a registry may hold.
///
/// Two `/`-separated segments of `[a-z0-9][a-z0-9-]*`. Three rules, each
/// load-bearing rather than tidy:
///
///   * **Lowercase**, because a case-insensitive filesystem would otherwise
///     make two names one directory, and a registry that means different things
///     on macOS and on Linux is a registry that cannot be checked.
///   * **Two segments**, because a bare name in a shared registry is a
///     landgrab. A path dependency may still be called anything; this is about
///     what may be published.
///   * **Neither `std` nor `ingot` as the owner**, because `Loader::follow`
///     tests `is_library` *first* -- such a package would install and then be
///     unreachable, which is the worst shape a failure can take.
///
/// Checked before the name is ever joined onto a path, because a name *is* a
/// path here and `..` is a name.
pub fn valid_name(name: str) bool {
    const parts = text.split(name, "/");
    if (array.len(parts) != 2) { return false; }
    if (text.eq(parts[0], "std") or text.eq(parts[0], "ingot")) { return false; }
    if (!valid_segment(parts[0])) { return false; }
    return valid_segment(parts[1]);
}

fn valid_segment(s: str) bool {
    const n = text.len(s);
    if (n == 0) { return false; }
    var i = 0;
    while (i < n) : (i += 1) {
        const c = text.byte_at(s, i);
        if (c >= 48 and c <= 57) { continue; }
        if (c >= 97 and c <= 122) { continue; }
        // A hyphen may join but may not lead, so that no name is a prefix
        // dressed up as a separator.
        if (c == 45 and i > 0) { continue; }
        return false;
    }
    return true;
}

/// Whether `s` is `n` lowercase hex characters and nothing else.
pub fn is_hex(s: str, n: i64) bool {
    if (text.len(s) != n) { return false; }
    var i = 0;
    while (i < n) : (i += 1) {
        const c = text.byte_at(s, i);
        if (c >= 48 and c <= 57) { continue; }
        if (c >= 97 and c <= 102) { continue; }
        return false;
    }
    return true;
}

/// Whether `tree` is the store key spelling: `sha256:` and 64 hex characters.
pub fn is_tree(tree: str) bool {
    if (!text.starts_with(tree, "sha256:")) { return false; }
    return is_hex(text.substr(tree, 7, text.len(tree)), 64);
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// Open a registry that is already on this machine.
pub fn open(f: fault.Fault, dir: str) ?Index {
    const file = path.join(dir, REGISTRY_NAME);
    if (!io.exists(file)) {
        fault.fail_at(f, dir, "is not a registry -- there is no Registry.toml in it");
        return null;
    }
    const doc = read_toml(f, file) orelse return null;
    const t = manifest.table_at(doc.root, "registry") orelse {
        fault.fail_at(f, file, "there is no `[registry]` table");
        return null;
    };
    const version = manifest.int_at(t, "version") orelse 0;
    if (version > INDEX_VERSION) {
        fault.fail_at(f, file, "was written by a newer ingot than this one");
        return null;
    }
    return Index{
        .dir = dir,
        .name = manifest.string_at(t, "name") orelse "the registry",
        .version = version,
    };
}

/// Where a package's two files live.
pub fn package_dir(ix: Index, name: str) str {
    return path.join(path.join(ix.dir, "packages"), name);
}

/// One package, or null.
///
/// **Null means two things, and the fault tells them apart.** A package this
/// registry simply does not hold is null with nothing said -- "which package is
/// missing, and what asked for it" is known one level up and is a much better
/// sentence than anything here could write. A package it holds and cannot read
/// is null with the fault set, at the file.
pub fn package(f: fault.Fault, ix: Index, name: str) ?Package {
    if (!valid_name(name)) { return null; }
    const dir = package_dir(ix, name);
    const file = path.join(dir, PACKAGE_NAME);
    if (!io.exists(file)) { return null; }

    const doc = read_toml(f, file) orelse return null;
    const t = manifest.table_at(doc.root, "package") orelse {
        fault.fail_at(f, file, "there is no `[package]` table");
        return null;
    };
    const spelled = manifest.string_at(t, "name") orelse {
        fault.fail_at(f, file, "`[package]` has no `name`");
        return null;
    };
    if (!text.eq(spelled, name)) {
        fault.fail_at(f, file, text.concat(text.concat("is called `", spelled),
            text.concat("`, and is filed under ", name)));
        return null;
    }
    const repo = manifest.string_at(t, "repo") orelse {
        fault.fail_at(f, file, "`[package]` has no `repo`");
        return null;
    };
    if (!text.starts_with(repo, "https://")) {
        fault.fail_at(f, file, "`repo` must be an `https://` URL");
        return null;
    }
    const releases = read_releases(f, path.join(dir, VERSIONS_NAME), name) orelse return null;
    return Package{ .name = name, .repo = repo, .releases = releases };
}

fn read_toml(f: fault.Fault, file: str) ?toml.Doc {
    const src = io.read_file(file) catch {
        fault.fail_at(f, file, "cannot be read");
        return null;
    };
    const doc = toml.parse(src);
    if (!doc.ok) {
        fault.fail_at(f, file, text.concat(text.concat("line ", text.from_int(doc.line)),
                                           text.concat(": ", doc.message)));
        return null;
    }
    return doc;
}

/// Every `[[version]]` in a `versions.toml`.
///
/// A package with no versions file has no versions, which is a package somebody
/// has reserved a name for rather than a broken one. Everything else is
/// checked here rather than trusted, because this file is written by whoever
/// opened the pull request and read by everyone.
fn read_releases(f: fault.Fault, file: str, name: str) ?list.List[Release] {
    var out: list.List[Release] = list.new();
    if (!io.exists(file)) { return out; }
    const doc = read_toml(f, file) orelse return null;

    var items: list.List[toml.Value] = list.new();
    if (toml.get(doc.root, "version")) |v| {
        items = toml.as_array(v) catch {
            fault.fail_at(f, file, "`version` must be written as `[[version]]` tables");
            return null;
        };
    }
    for (list.to_array(items)) |item| {
        const t = toml.as_table(item) catch {
            fault.fail_at(f, file, "every `[[version]]` must be a table");
            return null;
        };
        const spelled = manifest.string_at(t, "version") orelse {
            fault.fail_at(f, file, "a `[[version]]` has no `version`");
            return null;
        };
        const v = semver.parse(spelled) orelse {
            fault.fail_at(f, file, text.concat(text.concat("`", spelled), "` is not a version"));
            return null;
        };
        const rev = manifest.string_at(t, "rev") orelse "";
        if (!is_hex(rev, 40)) {
            fault.fail_at(f, file, text.concat(spelled,
                " needs a `rev` that is a full object id"));
            return null;
        }
        const tree = manifest.string_at(t, "tree") orelse "";
        if (!is_tree(tree)) {
            fault.fail_at(f, file, text.concat(spelled,
                " needs a `tree` written `sha256:` and 64 hex characters"));
            return null;
        }
        if (held(out, v)) {
            fault.fail_at(f, file, text.concat(spelled, " is written twice"));
            return null;
        }

        var needs: list.List[pubgrub.Need] = list.new();
        var deps = []str{};
        if (manifest.table_at(t, "dependencies")) |d| {
            for (toml.keys(d)) |key| {
                const req = manifest.string_at(d, key) orelse {
                    fault.fail_at(f, file, text.concat(text.concat(key,
                        " must be a version requirement, at "), spelled));
                    return null;
                };
                const range = semver.requirement(req) orelse {
                    fault.fail_at(f, file, text.concat(text.concat(text.concat("`", req),
                        "` is not a version requirement, wanted by "), text.concat(key,
                        text.concat(" at ", spelled))));
                    return null;
                };
                list.push(needs, pubgrub.Need{ .package = key, .range = range });
                deps = array.push(deps, key);
            }
        }
        list.push(out, Release{
            .version = v,
            .rev = rev,
            .tree = tree,
            .yanked = manifest.bool_at(t, "yanked") orelse false,
            .needs = needs,
            .deps = deps,
        });
    }
    return out;
}

fn held(l: list.List[Release], v: semver.Version) bool {
    for (list.to_array(l)) |r| {
        if (semver.eq(r.version, v)) { return true; }
    }
    return false;
}

/// One release of a package, yanked or not: `install` must be able to find a
/// version a lockfile already names.
pub fn release(p: Package, v: semver.Version) ?Release {
    for (list.to_array(p.releases)) |r| {
        if (semver.eq(r.version, v)) { return r; }
    }
    return null;
}

/// The version `ingot add` should record a caret on.
///
/// The highest that is neither yanked nor a pre-release. A pre-release is not a
/// candidate unless it was asked for, which is `pubgrub.choose`'s policy and is
/// the same answer here for the same reason: nobody typing `ingot add` meant
/// `1.1.0-rc.1`.
pub fn newest(p: Package) ?semver.Version {
    var found = false;
    var best = semver.zero();
    for (list.to_array(p.releases)) |r| {
        if (r.yanked) { continue; }
        if (text.len(r.version.pre) > 0) { continue; }
        if (found and semver.less(r.version, best)) { continue; }
        best = r.version;
        found = true;
    }
    if (!found) { return null; }
    return best;
}

/// Every package this registry holds, in byte order.
///
/// A directory holding a `package.toml` is a package; anything else under
/// `packages/` is a namespace. That rule is what lets the registry have no
/// central list of its contents -- a file every pull request would conflict on.
///
/// **Sorted, because `fs.read_dir` is not.** The order a filesystem lists a
/// directory in is neither sorted nor the same on two machines, so without this
/// `ingot search` answers in one order on ext4 and another on APFS -- which is
/// a difference somebody has to notice before they can distrust it. The store
/// sorts for the same reason one level down, and `store.sorted` is that sort.
pub fn names(ix: Index) []str {
    var out = []str{};
    const root = path.join(ix.dir, "packages");
    if (!fs.is_dir(root)) { return out; }
    const owners = fs.read_dir(root) catch return out;
    for (store.sorted(owners)) |owner| {
        const dir = path.join(root, owner);
        if (!fs.is_dir(dir)) { continue; }
        const held_by = fs.read_dir(dir) catch []str{};
        for (store.sorted(held_by)) |leaf| {
            if (!io.exists(path.join(path.join(dir, leaf), PACKAGE_NAME))) { continue; }
            out = array.push(out, text.concat(owner, text.concat("/", leaf)));
        }
    }
    return out;
}

// ---------------------------------------------------------------------------
// Getting one
// ---------------------------------------------------------------------------

/// Where the registry is: what `INGOT_REGISTRY` says, or the public one.
pub fn location() str {
    if (os.get(ENV_NAME)) |where| {
        if (text.len(where) > 0) { return where; }
    }
    return DEFAULT_URL;
}

/// Whether `where` is a registry already on this machine rather than a URL.
pub fn is_local(where: str) bool { return fs.is_dir(where); }

/// Fetch a registry's index into the store, and answer with the directory.
///
/// The first caller `git.discover` has ever had. A package is fetched by
/// revision because a manifest named one; a registry is fetched by *branch*,
/// because being current is the whole of its job -- so the reference list is
/// asked for first and `ref_id` turns the branch into the object id `fetch`
/// wants.
pub fn fetch(f: fault.Fault, h: str, url: str, cfg: tls.Config) ?Fetch {
    const remote = git.Remote{ .url = url, .cfg = cfg };
    const refs = git.discover(f, remote) orelse return null;
    const commit = git.ref_id(refs, DEFAULT_BRANCH) orelse {
        fault.fail(f, text.concat(url, text.concat(" has no ", DEFAULT_BRANCH)));
        return null;
    };
    const pack = git.fetch(f, remote, commit) orelse return null;
    const files = git.files(f, pack, commit) orelse return null;
    var carried: list.List[store.File] = list.new();
    for (list.to_array(files)) |file| {
        list.push(carried, store.File{ .path = file.path, .data = file.data });
    }
    const digest = store.install_files(f, h, carried);
    if (!f.ok) { return null; }
    store.remember_index(f, h, url, commit, digest);
    if (!f.ok) { return null; }
    return Fetch{ .commit = commit, .dir = store.entry(h, digest) };
}

/// What a fetch of an index answers with: the commit, and where it landed.
pub const Fetch = struct { commit: str, dir: str };

/// The directory holding the index for `where`.
///
/// A directory is itself; a URL is whatever was last fetched for it, and is
/// fetched now if nothing was and `may_fetch` allows it. `resolve` allows it so
/// that a first use needs no separate step, and `verify` does not, because
/// asking whether a project is ready must not go to the network.
pub fn directory(f: fault.Fault, h: str, where: str, cfg: tls.Config, may_fetch: bool) ?str {
    if (is_local(where)) { return where; }
    if (store.index_at(h, where)) |fetched| {
        if (store.check(h, fetched.digest) == store.READY) {
            return store.entry(h, fetched.digest);
        }
    }
    if (!may_fetch) {
        fault.fail(f, text.concat(text.concat("there is no index for ", where),
            " -- run `ingot update`"));
        return null;
    }
    const got = fetch(f, h, where, cfg) orelse return null;
    return got.dir;
}
