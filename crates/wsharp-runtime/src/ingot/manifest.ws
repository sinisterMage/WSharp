// `ingot.toml` and `ingot.lock`, read and written.
//
// Both are TOML, and both go through `std/toml` in both directions -- the
// lockfile especially, because a file written by a hand-rolled emitter and read
// by a real parser is a bug that waits for the one entry with a `"` in it.
//
// **The lockfile records the manifest's digest, not its timestamp.** Deciding
// whether a lock is stale by comparing modification times is what a store must
// not do: a checkout does not preserve them, two machines do not agree about
// them, and `git` will hand somebody a manifest older than the lock beside it.
// A hash answers the same question and cannot be wrong.
const array = @import("std/array");
const bytes = @import("std/bytes");
const fault = @import("ingot/fault");
const hash = @import("std/hash");
const io = @import("std/io");
const list = @import("std/list");
const path = @import("std/path");
const text = @import("std/str");
const toml = @import("std/toml");

/// One dependency, as the manifest asked for it.
///
/// Exactly one of `req`, `dir` and `git` says where it comes from. A registry
/// dependency has a version requirement; a path dependency has a directory; a
/// git dependency has a URL and a revision.
pub const Dep = struct { name: str, req: str, dir: str, git: str, rev: str };

pub const Manifest = struct {
    name: str,
    version: str,
    /// The file an `@import` of this package resolves to, relative to the
    /// manifest. The package's facade.
    root: str,
    deps: list.List[Dep],
    /// SHA-256 of the manifest's bytes, in hex. What the lockfile records so
    /// that `verify` can tell a stale lock from a current one.
    digest: str,
    /// The directory the manifest was read from, so a path dependency can be
    /// resolved relative to it.
    dir: str,
};

pub const MANIFEST_NAME = "ingot.toml";
pub const LOCK_NAME = "ingot.lock";

/// Read a manifest, or explain why not.
pub fn read(f: fault.Fault, file: str) ?Manifest {
    const src = io.read_file(file) catch {
        fault.fail_at(f, file, "cannot be read");
        return null;
    };
    return of_text(f, file, src, path.dirname(file));
}

/// The same, from text already in hand -- which is what a test uses, and what
/// a package already in the store is read through.
pub fn of_text(f: fault.Fault, where: str, src: str, dir: str) ?Manifest {
    const doc = toml.parse(src);
    if (!doc.ok) {
        fault.fail_at(f, where, text.concat(text.concat("line ", text.from_int(doc.line)),
                                            text.concat(": ", doc.message)));
        return null;
    }
    const pkg = table_at(doc.root, "package") orelse {
        fault.fail_at(f, where, "there is no `[package]` table");
        return null;
    };
    const name = string_at(pkg, "name") orelse {
        fault.fail_at(f, where, "`[package]` has no `name`");
        return null;
    };
    const version = string_at(pkg, "version") orelse {
        fault.fail_at(f, where, "`[package]` has no `version`");
        return null;
    };
    var root = string_at(pkg, "root") orelse "";
    if (text.len(root) == 0) { root = default_root(name); }

    var deps: list.List[Dep] = list.new();
    if (table_at(doc.root, "dependencies")) |t| {
        for (toml.keys(t)) |key| {
            const d = dependency(f, where, key, toml.get(t, key).?) orelse return null;
            list.push(deps, d);
        }
    }
    return Manifest{
        .name = name,
        .version = version,
        .root = root,
        .deps = deps,
        .digest = digest_of(src),
        .dir = dir,
    };
}

/// `acme/json` is `src/json.ws` unless the manifest says otherwise, so the
/// common package needs no `root` line.
fn default_root(name: str) str {
    const parts = text.split(name, "/");
    const last = parts[array.len(parts) - 1];
    return path.join("src", text.concat(last, ".ws"));
}

/// One `[dependencies]` entry, in either of its two spellings: a bare string
/// is a version requirement, and a table says where to get it instead.
fn dependency(f: fault.Fault, where: str, name: str, v: toml.Value) ?Dep {
    if (toml.kind(v) == toml.KIND_STR) {
        return Dep{
            .name = name,
            .req = toml.as_str(v) catch "",
            .dir = "",
            .git = "",
            .rev = "",
        };
    }
    const t = toml.as_table(v) catch {
        fault.fail_at(f, where, text.concat(text.concat("dependency `", name),
                                            "` must be a version or a table"));
        return null;
    };
    const dep = Dep{
        .name = name,
        .req = string_at(t, "version") orelse "",
        .dir = string_at(t, "path") orelse "",
        .git = string_at(t, "git") orelse "",
        .rev = string_at(t, "rev") orelse "",
    };
    var given = 0;
    if (text.len(dep.req) > 0) { given += 1; }
    if (text.len(dep.dir) > 0) { given += 1; }
    if (text.len(dep.git) > 0) { given += 1; }
    if (given != 1) {
        fault.fail_at(f, where, text.concat(text.concat("dependency `", name),
                                            "` needs exactly one of `version`, `path` and `git`"));
        return null;
    }
    if (text.len(dep.git) > 0 and text.len(dep.rev) == 0) {
        fault.fail_at(f, where, text.concat(text.concat("dependency `", name),
                                            "` is a git dependency and needs a `rev`"));
        return null;
    }
    return dep;
}

/// Render a manifest.
pub fn write(m: Manifest) str {
    const root = toml.table();
    const pkg = toml.table();
    toml.set(pkg, "name", toml.of_str(m.name));
    toml.set(pkg, "version", toml.of_str(m.version));
    toml.set(pkg, "root", toml.of_str(m.root));
    toml.set(root, "package", toml.of_table(pkg));

    const deps = toml.table();
    for (list.to_array(m.deps)) |d| {
        if (text.len(d.req) > 0 and text.len(d.dir) == 0 and text.len(d.git) == 0) {
            toml.set(deps, d.name, toml.of_str(d.req));
        } else {
            const t = toml.table();
            t.braced = true;
            if (text.len(d.dir) > 0) { toml.set(t, "path", toml.of_str(d.dir)); }
            if (text.len(d.git) > 0) {
                toml.set(t, "git", toml.of_str(d.git));
                toml.set(t, "rev", toml.of_str(d.rev));
            }
            toml.set(deps, d.name, toml.of_table(t));
        }
    }
    toml.set(root, "dependencies", toml.of_table(deps));
    return toml.write(root);
}

// ---------------------------------------------------------------------------
// The lockfile
// ---------------------------------------------------------------------------

/// One package, as resolving settled it.
pub const Locked = struct {
    name: str,
    version: str,
    /// Where it came from, in one string: `path+../util`, or
    /// `git+https://host/repo#<rev>`.
    source: str,
    /// The store key, `sha256:<hex>`, or `""` for a path dependency, which is
    /// used where it lies and never copied in.
    tree: str,
    deps: []str,
};

pub const Lock = struct {
    version: i64,
    /// The digest of the manifest this was resolved from.
    manifest: str,
    packages: list.List[Locked],
};

/// The format this version of ingot writes. A lockfile from a newer one is
/// refused rather than guessed at.
pub const LOCK_VERSION = 1;

pub fn read_lock(f: fault.Fault, file: str) ?Lock {
    const src = io.read_file(file) catch {
        fault.fail_at(f, file, "cannot be read");
        return null;
    };
    return lock_of_text(f, file, src);
}

/// The same, from text already in hand -- which is what lets the round trip
/// through `write_lock` be a test rather than a hope.
pub fn lock_of_text(f: fault.Fault, where: str, src: str) ?Lock {
    const doc = toml.parse(src);
    if (!doc.ok) {
        fault.fail_at(f, where, text.concat(text.concat("line ", text.from_int(doc.line)),
                                            text.concat(": ", doc.message)));
        return null;
    }
    const version = int_at(doc.root, "version") orelse 0;
    if (version > LOCK_VERSION) {
        fault.fail_at(f, where, "was written by a newer ingot than this one");
        return null;
    }
    var packages: list.List[Locked] = list.new();
    if (toml.get(doc.root, "package")) |v| {
        var empty: list.List[toml.Value] = list.new();
        for (list.to_array(toml.as_array(v) catch empty)) |item| {
            const t = toml.as_table(item) catch {
                fault.fail_at(f, where, "`[[package]]` must be a table");
                return null;
            };
            list.push(packages, Locked{
                .name = string_at(t, "name") orelse "",
                .version = string_at(t, "version") orelse "",
                .source = string_at(t, "source") orelse "",
                .tree = string_at(t, "tree") orelse "",
                .deps = strings_at(t, "dependencies"),
            });
        }
    }
    return Lock{
        .version = version,
        .manifest = string_at(doc.root, "manifest") orelse "",
        .packages = packages,
    };
}

pub fn write_lock(l: Lock) str {
    const root = toml.table();
    toml.set(root, "version", toml.of_int(LOCK_VERSION));
    toml.set(root, "manifest", toml.of_str(l.manifest));
    var items: list.List[toml.Value] = list.new();
    for (list.to_array(l.packages)) |p| {
        const t = toml.table();
        toml.set(t, "name", toml.of_str(p.name));
        toml.set(t, "version", toml.of_str(p.version));
        toml.set(t, "source", toml.of_str(p.source));
        toml.set(t, "tree", toml.of_str(p.tree));
        var deps: list.List[toml.Value] = list.new();
        for (p.deps) |d| { list.push(deps, toml.of_str(d)); }
        toml.set(t, "dependencies", toml.of_array(deps, true));
        list.push(items, toml.of_table(t));
    }
    toml.set(root, "package", toml.of_array(items, false));
    return toml.write(root);
}

/// The entry for `name`, or null.
pub fn locked(l: Lock, name: str) ?Locked {
    for (list.to_array(l.packages)) |p| {
        if (text.eq(p.name, name)) { return p; }
    }
    return null;
}

// ---------------------------------------------------------------------------
// Reading a TOML table without a `catch` at every step
// ---------------------------------------------------------------------------

pub fn string_at(t: toml.Table, key: str) ?str {
    const v = toml.get(t, key) orelse return null;
    return toml.as_str(v) catch return null;
}

pub fn int_at(t: toml.Table, key: str) ?i64 {
    const v = toml.get(t, key) orelse return null;
    return toml.as_int(v) catch return null;
}

pub fn table_at(t: toml.Table, key: str) ?toml.Table {
    const v = toml.get(t, key) orelse return null;
    return toml.as_table(v) catch return null;
}

/// An array of strings, or an empty one. A missing list and an empty list mean
/// the same thing everywhere in these two files.
pub fn strings_at(t: toml.Table, key: str) []str {
    const v = toml.get(t, key) orelse return []str{};
    var empty: list.List[toml.Value] = list.new();
    const items = toml.as_array(v) catch empty;
    var out = []str{};
    for (list.to_array(items)) |item| {
        out = array.push(out, toml.as_str(item) catch "");
    }
    return out;
}

/// SHA-256 of some text, in hex.
pub fn digest_of(src: str) str {
    const b = bytes.of(src);
    return bytes.to_hex(hash.sha256(b));
}
