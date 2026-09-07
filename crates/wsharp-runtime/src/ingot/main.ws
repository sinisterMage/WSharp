// ingot: the verbs.
//
// The name is item 9's joke made load-bearing -- C#'s package manager is NuGet,
// and in Minecraft nine gold nuggets craft one ingot.
//
// Modelled on Ajt, for the parts that are about being a package manager rather
// than about Julia:
//
//   * **Resolving, installing and building are separate verbs.** Nothing
//     compiles because something else was fetched. `add` records an intent,
//     `resolve` chooses versions and writes a lockfile, `install` makes the
//     store satisfy that lockfile, and `build` is a thing you asked for.
//   * **`verify` answers with its exit status** -- ready, needs installing,
//     needs resolving, broken -- so a script can ask without parsing anything.
//   * **Output is tab-separated**, so a shell can cut it up.
//   * **`why` prints the paths that explain an entry**, because "what pulled
//     this in" is the question a lockfile never answers on its own.
//
// A path dependency is *copied into the store* rather than used where it lies,
// which is the one place this differs from what most package managers do. It
// follows from the first principle above: if a build read a dependency's
// working directory, then editing that directory would change what the build
// does without anything having been resolved or installed, and the separation
// the other three verbs exist for would be a fiction.
const array = @import("std/array");
const bytes = @import("std/bytes");
const fault = @import("ingot/fault");
const fs = @import("std/fs");
const io = @import("std/io");
const list = @import("std/list");
const manifest = @import("ingot/manifest");
const os = @import("std/os");
const path = @import("std/path");
const plan = @import("ingot/plan");
const store = @import("ingot/store");
const text = @import("std/str");
const toml = @import("std/toml");

pub const OK = 0;
pub const NEEDS_INSTALLING = 1;
pub const NEEDS_RESOLVING = 2;
pub const BROKEN = 3;
/// What a verb that could not be carried out at all exits with. Distinct from
/// `verify`'s answers, which are about a project rather than about a mistake.
pub const FAILED = 4;

fn main() i64 {
    const args = os.args();
    if (array.len(args) == 0) { usage(); return FAILED; }
    const verb = args[0];
    const rest = array.slice(args, 1, array.len(args));

    const f = fault.none();
    var code = FAILED;
    if (text.eq(verb, "init")) { code = init(f, rest); }
    else if (text.eq(verb, "add")) { code = add(f, rest); }
    else if (text.eq(verb, "remove")) { code = remove(f, rest); }
    else if (text.eq(verb, "resolve")) { code = resolve(f); }
    else if (text.eq(verb, "install")) { code = install(f); }
    else if (text.eq(verb, "verify")) { code = verify(f); }
    else if (text.eq(verb, "why")) { code = why(f, rest); }
    else if (text.eq(verb, "list")) { code = show_list(f); }
    else if (text.eq(verb, "gc")) { code = gc(f); }
    else if (text.eq(verb, "store")) { code = show_store(f); }
    else if (text.eq(verb, "help")) { usage(); code = OK; }
    else {
        fault.fail(f, text.concat(text.concat("no such verb: `", verb), "`"));
    }
    if (!f.ok) {
        print(text.concat("ingot: ", f.message));
        if (code == OK) { code = FAILED; }
    }
    return code;
}

fn usage() void {
    print("ingot -- the W# package manager");
    print("");
    print("  init [name]              write an ingot.toml here");
    print("  add <name> <version>     record a dependency");
    print("  add <name> --path <dir>  record a dependency on a directory");
    print("  remove <name>            take one out");
    print("  resolve                  choose versions and write ingot.lock");
    print("  install                  make the store satisfy ingot.lock");
    print("  verify                   0 ready, 1 install, 2 resolve, 3 broken");
    print("  list                     what the lockfile holds");
    print("  why <name>               the paths that pulled it in");
    print("  gc                       drop store entries nothing reaches");
    print("  store                    where the store is, and what is in it");
    print("  run <file.ws> [args]     compile and run a program");
    print("");
    print("`-C <dir>` works from somewhere else. `--gc-stress` belongs to what");
    print("is being run. Output is tab-separated, and WSHARP_HOME says where");
    print("the store is.");
    return;
}

// ---------------------------------------------------------------------------
// Where we are
// ---------------------------------------------------------------------------

fn manifest_path() str { return manifest.MANIFEST_NAME; }
fn lock_path() str { return manifest.LOCK_NAME; }

fn read_here(f: fault.Fault) ?manifest.Manifest {
    if (!io.exists(manifest_path())) {
        fault.fail(f, "there is no ingot.toml here -- `ingot init` writes one");
        return null;
    }
    return manifest.read(f, manifest_path());
}

// ---------------------------------------------------------------------------
// init, add, remove
// ---------------------------------------------------------------------------

fn init(f: fault.Fault, args: []str) i64 {
    if (io.exists(manifest_path())) {
        fault.fail(f, "there is already an ingot.toml here");
        return FAILED;
    }
    var name = "a-package";
    if (array.len(args) > 0) { name = args[0]; }
    var deps: list.List[manifest.Dep] = list.new();
    const m = manifest.Manifest{
        .name = name,
        .version = "0.1.0",
        .root = default_root(name),
        .deps = deps,
        .digest = "",
        .dir = ".",
    };
    const body = manifest.write(m);
    io.write_file(manifest_path(), body) catch {
        fault.fail_at(f, manifest_path(), "cannot be written");
        return FAILED;
    };
    print(row2("wrote", manifest_path()));
    return OK;
}

fn default_root(name: str) str {
    const parts = text.split(name, "/");
    return path.join("src", text.concat(parts[array.len(parts) - 1], ".ws"));
}

fn add(f: fault.Fault, args: []str) i64 {
    if (array.len(args) == 0) {
        fault.fail(f, "`add` needs a package name");
        return FAILED;
    }
    const m = read_here(f) orelse return FAILED;
    const name = args[0];
    const d = new_dependency(f, name, array.slice(args, 1, array.len(args))) orelse return FAILED;
    var kept: list.List[manifest.Dep] = list.new();
    for (list.to_array(m.deps)) |old| {
        if (!text.eq(old.name, name)) { list.push(kept, old); }
    }
    list.push(kept, d);
    m.deps = kept;
    write_manifest(f, m);
    if (!f.ok) { return FAILED; }
    print(row2("added", name));
    // The lockfile no longer describes the manifest, and saying so here is what
    // stops `install` being run against a stale one.
    if (io.exists(lock_path())) { print(row2("stale", lock_path())); }
    return OK;
}

fn new_dependency(f: fault.Fault, name: str, rest: []str) ?manifest.Dep {
    const n = array.len(rest);
    if (n == 1 and !text.starts_with(rest[0], "--")) {
        return manifest.Dep{ .name = name, .req = rest[0], .dir = "", .git = "", .rev = "" };
    }
    var req = "";
    var dir = "";
    var git = "";
    var rev = "";
    var i = 0;
    while (i < n) : (i += 1) {
        const flag = rest[i];
        if (i + 1 >= n) {
            fault.fail(f, text.concat(flag, " needs a value"));
            return null;
        }
        const value = rest[i + 1];
        i += 1;
        if (text.eq(flag, "--path")) { dir = value; continue; }
        if (text.eq(flag, "--git")) { git = value; continue; }
        if (text.eq(flag, "--rev")) { rev = value; continue; }
        if (text.eq(flag, "--version")) { req = value; continue; }
        fault.fail(f, text.concat(text.concat("no such option: `", flag), "`"));
        return null;
    }
    var given = 0;
    if (text.len(req) > 0) { given += 1; }
    if (text.len(dir) > 0) { given += 1; }
    if (text.len(git) > 0) { given += 1; }
    if (given != 1) {
        fault.fail(f, "a dependency needs exactly one of a version, `--path` and `--git`");
        return null;
    }
    return manifest.Dep{ .name = name, .req = req, .dir = dir, .git = git, .rev = rev };
}

fn remove(f: fault.Fault, args: []str) i64 {
    if (array.len(args) == 0) {
        fault.fail(f, "`remove` needs a package name");
        return FAILED;
    }
    const m = read_here(f) orelse return FAILED;
    const name = args[0];
    var kept: list.List[manifest.Dep] = list.new();
    var found = false;
    for (list.to_array(m.deps)) |d| {
        if (text.eq(d.name, name)) { found = true; } else { list.push(kept, d); }
    }
    if (!found) {
        fault.fail(f, text.concat(text.concat("`", name), "` is not a dependency of this package"));
        return FAILED;
    }
    m.deps = kept;
    write_manifest(f, m);
    if (!f.ok) { return FAILED; }
    print(row2("removed", name));
    return OK;
}

fn write_manifest(f: fault.Fault, m: manifest.Manifest) void {
    io.write_file(manifest_path(), manifest.write(m))
        catch fault.fail_at(f, manifest_path(), "cannot be written");
    return;
}

// ---------------------------------------------------------------------------
// resolve
// ---------------------------------------------------------------------------

/// Choose versions for everything the manifest asks for, and write them down.
///
/// The work is `ingot/plan`; this is the verb. A failure that is the solver's
/// is printed as the solver wrote it -- a derivation naming the line of the
/// manifest to change -- rather than being folded into one of ours.
fn resolve(f: fault.Fault) i64 {
    const m = read_here(f) orelse return FAILED;
    const outcome = plan.resolve(f, m);
    const lock = outcome.lock orelse {
        if (text.len(outcome.report) > 0) { print(outcome.report); }
        return FAILED;
    };
    io.write_file(lock_path(), manifest.write_lock(lock)) catch {
        fault.fail_at(f, lock_path(), "cannot be written");
        return FAILED;
    };
    print(row2("resolved", text.from_int(list.len(lock.packages))));
    return OK;
}

fn seen(packages: list.List[manifest.Locked], name: str) ?manifest.Locked {
    for (list.to_array(packages)) |p| {
        if (text.eq(p.name, name)) { return p; }
    }
    return null;
}

// ---------------------------------------------------------------------------
// install, verify
// ---------------------------------------------------------------------------

fn install(f: fault.Fault) i64 {
    const m = read_here(f) orelse return FAILED;
    const lock = read_lock_here(f) orelse return FAILED;
    if (!text.eq(lock.manifest, m.digest)) {
        fault.fail(f, "ingot.lock does not describe this ingot.toml -- run `ingot resolve`");
        return NEEDS_RESOLVING;
    }
    const h = store.home() catch {
        fault.fail(f, "cannot work out where the store is: neither WSHARP_HOME nor a home directory");
        return FAILED;
    };
    var installed = 0;
    for (list.to_array(lock.packages)) |p| {
        const digest = digest_of(p.tree);
        if (store.check(h, digest) == store.READY) { continue; }
        const dir = source_dir(p) orelse {
            fault.fail(f, text.concat(text.concat("`", p.name), "` has a source ingot cannot fetch"));
            return FAILED;
        };
        const got = store.install(f, h, dir);
        if (!f.ok) { return FAILED; }
        if (!text.eq(got, digest)) {
            fault.fail(f, text.concat(text.concat("`", p.name),
                "` has changed since it was resolved -- run `ingot resolve`"));
            return NEEDS_RESOLVING;
        }
        print(row3("installed", p.name, digest));
        installed += 1;
    }
    store.register(f, h, lock_path());
    if (!f.ok) { return FAILED; }
    print(row2("ready", text.from_int(list.len(lock.packages))));
    return OK;
}

/// Ready, or the smallest thing that has to happen first.
///
/// The exit status is the answer, so a script can ask without reading a word
/// of this. Every check is done rather than the first failure returned, because
/// "resolve, then install" is more useful to be told at once.
fn verify(f: fault.Fault) i64 {
    const m = read_here(f) orelse return BROKEN;
    if (!io.exists(lock_path())) {
        print(row2("needs", "resolve"));
        return NEEDS_RESOLVING;
    }
    const lock = read_lock_here(f) orelse return BROKEN;
    if (!text.eq(lock.manifest, m.digest)) {
        print(row3("stale", lock_path(), "ingot.toml has changed"));
        return NEEDS_RESOLVING;
    }
    const h = store.home() catch {
        fault.fail(f, "cannot work out where the store is");
        return BROKEN;
    };
    var worst = OK;
    for (list.to_array(lock.packages)) |p| {
        const digest = digest_of(p.tree);
        const state = store.check(h, digest);
        if (state == store.DAMAGED) {
            print(row3("damaged", p.name, digest));
            worst = BROKEN;
            continue;
        }
        if (state == store.MISSING) {
            print(row3("missing", p.name, digest));
            if (worst < NEEDS_INSTALLING) { worst = NEEDS_INSTALLING; }
            continue;
        }
        // A path dependency that has been edited since it was resolved is
        // neither missing nor damaged: the store holds exactly what it was
        // told to. It is the *lockfile* that is out of date.
        if (source_dir(p)) |dir| {
            if (fs.is_dir(dir)) {
                const now = store.tree_hash(f, dir);
                if (f.ok and !text.eq(now, digest)) {
                    print(row3("changed", p.name, dir));
                    if (worst < NEEDS_RESOLVING) { worst = NEEDS_RESOLVING; }
                }
                f.ok = true;
            }
        }
    }
    if (worst == OK) { print(row2("ready", text.from_int(list.len(lock.packages)))); }
    return worst;
}

fn read_lock_here(f: fault.Fault) ?manifest.Lock {
    if (!io.exists(lock_path())) {
        fault.fail(f, "there is no ingot.lock here -- run `ingot resolve`");
        return null;
    }
    return manifest.read_lock(f, lock_path());
}

/// `sha256:<hex>` back to the hex.
fn digest_of(tree: str) str {
    const at = text.find(tree, ":");
    if (at < 0) { return tree; }
    return text.substr(tree, at + 1, text.len(tree));
}

/// Where a locked package's source is, for the sources ingot can reach.
fn source_dir(p: manifest.Locked) ?str {
    if (text.starts_with(p.source, "path+")) {
        return text.substr(p.source, 5, text.len(p.source));
    }
    return null;
}

// ---------------------------------------------------------------------------
// list, why
// ---------------------------------------------------------------------------

fn show_list(f: fault.Fault) i64 {
    const lock = read_lock_here(f) orelse return FAILED;
    for (list.to_array(lock.packages)) |p| {
        print(row4(p.name, p.version, p.source, p.tree));
    }
    return OK;
}

/// Every path from the root package to `name`.
///
/// The question a lockfile never answers on its own. Breadth first, so the
/// shortest explanation comes first, and every path is printed rather than one,
/// because "it is needed twice" is usually the answer somebody wanted.
fn why(f: fault.Fault, args: []str) i64 {
    if (array.len(args) == 0) {
        fault.fail(f, "`why` needs a package name");
        return FAILED;
    }
    const want = args[0];
    const m = read_here(f) orelse return FAILED;
    const lock = read_lock_here(f) orelse return FAILED;

    var paths: list.List[str] = list.new();
    var frontier: list.List[str] = list.new();
    for (list.to_array(m.deps)) |d| { list.push(frontier, d.name); }

    var found = 0;
    var depth = 0;
    while (list.len(frontier) > 0 and depth < 64) : (depth += 1) {
        var next: list.List[str] = list.new();
        var i = 0;
        while (i < list.len(frontier)) : (i += 1) {
            const trail = list.get(frontier, i);
            const last = last_step(trail);
            if (text.eq(last, want)) {
                print(text.concat(m.name, text.concat("\t", trail)));
                found += 1;
                continue;
            }
            const p = seen(lock.packages, last) orelse continue;
            for (p.deps) |child| {
                // A cycle would loop for ever; a name already on this path has
                // nothing further to explain.
                if (!on_path(trail, child)) {
                    list.push(next, text.concat(trail, text.concat("\t", child)));
                }
            }
        }
        frontier = next;
    }
    if (found == 0) {
        print(row2("nothing", want));
        return NEEDS_INSTALLING;
    }
    return OK;
}

fn last_step(trail: str) str {
    const parts = text.split(trail, "\t");
    return parts[array.len(parts) - 1];
}

fn on_path(trail: str, name: str) bool {
    for (text.split(trail, "\t")) |step| {
        if (text.eq(step, name)) { return true; }
    }
    return false;
}

// ---------------------------------------------------------------------------
// gc, store
// ---------------------------------------------------------------------------

fn gc(f: fault.Fault) i64 {
    const h = store.home() catch {
        fault.fail(f, "cannot work out where the store is");
        return FAILED;
    };
    var keep: list.List[str] = list.new();
    for (store.registered(h)) |file| {
        const lock = manifest.read_lock(f, file) orelse {
            // A lockfile this ingot cannot read keeps nothing alive, and
            // refusing to collect because of one is how a disk fills.
            f.ok = true;
            f.message = "";
            continue;
        };
        for (list.to_array(lock.packages)) |p| { list.push(keep, digest_of(p.tree)); }
    }
    const removed = store.collect(f, h, keep);
    if (!f.ok) { return FAILED; }
    print(row2("removed", text.from_int(removed)));
    print(row2("kept", text.from_int(list.len(keep))));
    return OK;
}

fn show_store(f: fault.Fault) i64 {
    const h = store.home() catch {
        fault.fail(f, "cannot work out where the store is");
        return FAILED;
    };
    print(row2("home", h));
    print(row2("objects", store.objects(h)));
    var entries = 0;
    if (fs.is_dir(store.objects(h))) {
        entries = array.len(fs.read_dir(store.objects(h)) catch []str{});
    }
    print(row2("entries", text.from_int(entries)));
    print(row2("environments", text.from_int(array.len(store.registered(h)))));
    return OK;
}

// ---------------------------------------------------------------------------
// Tab-separated output
// ---------------------------------------------------------------------------

fn row2(a: str, b: str) str { return text.concat(a, text.concat("\t", b)); }
fn row3(a: str, b: str, c: str) str { return row2(a, row2(b, c)); }
fn row4(a: str, b: str, c: str, d: str) str { return row2(a, row3(b, c, d)); }
