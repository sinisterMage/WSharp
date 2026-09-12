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
const progress = @import("ingot/progress");
const registry = @import("ingot/registry");
const semver = @import("ingot/semver");
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
    // `--gc-stress` belongs to whatever is being run rather than to a verb, so
    // it is taken out wherever it appears and handed to `run` if that is where
    // we end up. No verb of ours has an opinion about it.
    const raw = os.args();
    const stress = mentions(raw, "--gc-stress");
    var args = without(raw, "--gc-stress");

    // `-C <dir>` works from somewhere else, as `git -C` does. First position
    // only, and handled here rather than in a verb because a process has one
    // working directory: changing it half way through would be a thing every
    // path in every verb had to know about.
    if (array.len(args) >= 1 and text.eq(args[0], "-C")) {
        if (array.len(args) < 2) {
            print_err("ingot: `-C` needs a directory");
            return FAILED;
        }
        os.chdir(args[1]) catch {
            print_err(text.concat("ingot: cannot work in ", args[1]));
            return FAILED;
        };
        args = array.slice(args, 2, array.len(args));
    }

    if (array.len(args) == 0) { usage(); return FAILED; }
    // A program's own arguments belong to it. Only ingot verbs consume quiet.
    var leading_quiet = false;
    while (array.len(args) > 0) {
        if (!text.eq(args[0], "--quiet") and !text.eq(args[0], "-q")) { break; }
        leading_quiet = true;
        args = array.slice(args, 1, array.len(args));
    }
    if (array.len(args) == 0) { usage(); return FAILED; }
    if (!text.eq(args[0], "run")) {
        const quiet = leading_quiet or mentions(args, "--quiet") or mentions(args, "-q");
        args = without(without(args, "--quiet"), "-q");
        if (array.len(args) == 0) { usage(); return FAILED; }
        return command(args, quiet, stress);
    }
    return command(args, leading_quiet, stress);
}

fn command(args: []str, quiet: bool, stress: bool) i64 {
    const verb = args[0];
    const rest = array.slice(args, 1, array.len(args));

    // The one verb that is the compiler, and so the one that is not ours to
    // carry out: this binary is a W# program and has no compiler inside it.
    // It becomes `wsharp run` instead, which is the same JIT by the same name.
    if (text.eq(verb, "run")) { return run_program(rest, stress); }

    const ui = progress.new(quiet);
    const f = fault.reporting(fn(message: str) void { progress.status(ui, message); return; });
    var code = FAILED;
    if (text.eq(verb, "init")) { code = init(f, rest); }
    else if (text.eq(verb, "add")) { code = add(f, rest); }
    else if (text.eq(verb, "remove")) { code = remove(f, rest); }
    else if (text.eq(verb, "update")) { code = update(f); }
    else if (text.eq(verb, "search")) { code = search(f, rest); }
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
    progress.finish(ui, f.ok and code == OK);
    if (!f.ok) {
        // The error stream, so that a shell redirecting a verb's output still
        // sees why it got none. Every verb's *answer* is on stdout.
        print_err(text.concat("ingot: ", f.message));
        if (code == OK) { code = FAILED; }
    }
    return code;
}

// ---------------------------------------------------------------------------
// run, which is the compiler's
// ---------------------------------------------------------------------------

/// `ingot run <file.ws> [args]`, by becoming `wsharp run`.
///
/// This used to be a Rust driver's, and for a good reason: the binary embedded
/// the compiler and a W# program has no way to ask for one. It still has no
/// way, so the answer is not to embed it but to hand over -- `exec` replaces
/// this process with the compiler, which JITs and runs the program exactly as
/// it always did. The user sees one process and one exit status either way.
///
/// Answers only if `exec` fails, because on success there is nobody here to
/// answer.
fn run_program(rest: []str, stress: bool) i64 {
    if (array.len(rest) == 0) {
        print_err("ingot: `run` needs a file");
        return FAILED;
    }
    // A leading `--` is this driver's punctuation, not the program's first
    // argument -- the same rule `wsharp run` applies.
    var forward = []str{ "run" };
    if (stress) { forward = array.push(forward, "--gc-stress"); }
    forward = array.concat(forward, rest);

    const wsharp = compiler();
    os.exec(wsharp, forward) catch {
        print_err(text.concat("ingot: cannot run the compiler at ", wsharp));
        return FAILED;
    };
    // `exec` does not come back, so reaching here is itself the failure.
    return FAILED;
}

/// Where the compiler is.
///
/// Beside this binary if it is there, because that is how a release is laid
/// out and it is the answer that keeps a checkout and an installation from
/// disagreeing. Otherwise the bare name, which lets the system search `PATH`
/// -- and which is also the answer on a system that cannot say where a running
/// program lives.
fn compiler() str {
    const me = os.self_exe() catch "";
    if (text.len(me) > 0) {
        const dir = path.dirname(path.normalise(me));
        // `.exe` on Windows, and nothing anywhere else. Asked by looking
        // rather than by knowing the platform, which this language has no way
        // to ask about and should not need to.
        if (io.exists(path.join(dir, "wsharp.exe"))) {
            return path.join(dir, "wsharp.exe");
        }
        if (io.exists(path.join(dir, "wsharp"))) {
            return path.join(dir, "wsharp");
        }
    }
    return "wsharp";
}

// ---------------------------------------------------------------------------
// The command line, before a verb sees it
// ---------------------------------------------------------------------------

/// Whether `flag` appears anywhere in `args`.
fn mentions(args: []str, flag: str) bool {
    const n = array.len(args);
    var i = 0;
    while (i < n) : (i += 1) {
        if (text.eq(args[i], flag)) { return true; }
    }
    return false;
}

/// `args` without any occurrence of `flag`.
///
/// Removed wherever it appears rather than only in front, because it is not a
/// verb's argument and a user who writes it last means the same thing.
fn without(args: []str, flag: str) []str {
    const n = array.len(args);
    var out = []str{};
    var i = 0;
    while (i < n) : (i += 1) {
        if (!text.eq(args[i], flag)) { out = array.push(out, args[i]); }
    }
    return out;
}

fn usage() void {
    print("ingot -- the W# package manager");
    print("");
    print("  init [name]              write an ingot.toml here");
    print("  add <name> [version]     record a dependency, newest by default");
    print("  add <name> --path <dir>  record a dependency on a directory");
    print("  remove <name>            take one out");
    print("  update                   fetch the registry index again");
    print("  search [text]            what the registry holds");
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
    print("is being run, so `run` passes it on and nothing else reads it.");
    print("Output is tab-separated, and WSHARP_HOME says where the store is.");
    print("Progress goes to stderr; `--quiet` (or `-q`) hides it.");
    print("");
    print("INGOT_REGISTRY says which registry to use, as a URL or as a");
    print("directory. A directory is used where it lies and is never fetched,");
    print("which is what a private or an offline registry is.");
    print("");
    print("`run` hands over to `wsharp`, which is looked for beside this");
    print("program and then on PATH: this binary is a W# program and the");
    print("compiler is what compiles W#.");
    print("");
    print("`install` also writes ingot.env, which is how the compiler finds a");
    print("package's files. It holds absolute paths, so it is derived rather");
    print("than committed -- `install` writes it again whenever it is missing.");
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
    answer(f, row2("wrote", manifest_path()));
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
    var rest = array.slice(args, 1, array.len(args));
    // `ingot add acme/json` means the newest the registry has, which is the
    // spelling people reach for. Asked here rather than in the parser below,
    // because it is the one form that needs an index -- every other spelling of
    // `add` is an edit to a file and touches nothing else.
    if (array.len(rest) == 0) {
        const req = newest_requirement(f, name) orelse return FAILED;
        rest = []str{ req };
    }
    const d = new_dependency(f, name, rest) orelse return FAILED;
    var kept: list.List[manifest.Dep] = list.new();
    for (list.to_array(m.deps)) |old| {
        if (!text.eq(old.name, name)) { list.push(kept, old); }
    }
    list.push(kept, d);
    m.deps = kept;
    write_manifest(f, m);
    if (!f.ok) { return FAILED; }
    answer(f, row2("added", name));
    // The lockfile no longer describes the manifest, and saying so here is what
    // stops `install` being run against a stale one.
    if (io.exists(lock_path())) { answer(f, row2("stale", lock_path())); }
    return OK;
}

/// `^<newest>`, out of the registry.
///
/// A caret rather than an exact version, because recording what is newest today
/// as what this package *requires* is how a project ends up unable to share a
/// dependency with anything else.
fn newest_requirement(f: fault.Fault, name: str) ?str {
    fault.status(f, text.concat("Looking up ", name));
    const net = plan.network();
    const ix = plan.index(f, net) orelse return null;
    const p = registry.package(f, ix, name) orelse {
        // Null and no fault means it is not in the registry; null with one
        // means the entry is unreadable and has already said so.
        if (f.ok) {
            fault.fail(f, text.concat(text.concat("`", name),
                text.concat("` is not in ", text.concat(ix.name,
                    " -- give a version, a `--path` or a `--git` instead"))));
        }
        return null;
    };
    const v = registry.newest(p) orelse {
        fault.fail(f, text.concat(text.concat("`", name),
            "` has no version that is not yanked or a pre-release"));
        return null;
    };
    return text.concat("^", semver.render(v));
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
    answer(f, row2("removed", name));
    return OK;
}

fn write_manifest(f: fault.Fault, m: manifest.Manifest) void {
    io.write_file(manifest_path(), manifest.write(m))
        catch fault.fail_at(f, manifest_path(), "cannot be written");
    return;
}

// ---------------------------------------------------------------------------
// update, search
// ---------------------------------------------------------------------------

/// Fetch the registry index again.
///
/// `resolve` uses whatever index is already here, because refetching one per
/// build is a network round trip on every build. Being current is therefore a
/// thing you ask for, and this is the asking.
fn update(f: fault.Fault) i64 {
    fault.status(f, "Updating registry index");
    const where = registry.location();
    // A directory is already the index. Fetching nothing and reporting a commit
    // that does not exist would be the wrong kind of success.
    if (registry.is_local(where)) {
        const ix = registry.open(f, where) orelse return FAILED;
        answer(f, row3("local", ix.name, where));
        return OK;
    }
    const got = plan.update_index(f, plan.network()) orelse return FAILED;
    answer(f, row3("updated", where, got.commit));
    return OK;
}

/// What the registry holds, filtered by a substring of the name.
fn search(f: fault.Fault, args: []str) i64 {
    var needle = "";
    if (array.len(args) > 0) { needle = args[0]; }
    const ix = plan.index(f, plan.network()) orelse return FAILED;
    var found = 0;
    for (registry.names(ix)) |name| {
        if (text.len(needle) > 0 and text.find(name, needle) < 0) { continue; }
        const p = registry.package(f, ix, name) orelse {
            // One unreadable entry is one entry, not the end of a search: the
            // rule a trust store follows, for the same reason. A chain is a
            // structure and a bag is a bag.
            f.ok = true;
            f.message = "";
            continue;
        };
        // `-` rather than nothing, because a row whose last field is empty is a
        // row with trailing whitespace, which the case harness trims away.
        var newest = "-";
        if (registry.newest(p)) |v| { newest = semver.render(v); }
        answer(f, row3(name, newest, p.repo));
        found += 1;
    }
    answer(f, row2("found", text.from_int(found)));
    return OK;
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
    fault.status(f, "Resolving dependencies");
    const outcome = plan.resolve(f, plan.network(), m);
    const lock = outcome.lock orelse {
        // The solver's account of why, on the error stream with every other
        // explanation of a failure. Stdout carries what a verb *achieved* --
        // here, nothing -- and a script cutting up `resolved<TAB>n` should not
        // have to tell that apart from a page of reasoning.
        if (text.len(outcome.report) > 0) { fault.flush(f); print_err(outcome.report); }
        return FAILED;
    };
    io.write_file(lock_path(), manifest.write_lock(lock)) catch {
        fault.fail_at(f, lock_path(), "cannot be written");
        return FAILED;
    };
    // The environment describes a resolution that is no longer this one, and a
    // compiler reading a stale one would build against the versions chosen last
    // time -- silently, since those store entries are still there and still
    // hold what they always did. Removed rather than rewritten, because where
    // things land is `install`'s answer and not this verb's.
    if (io.exists(manifest.ENV_NAME)) {
        fs.remove(manifest.ENV_NAME) catch {
            fault.fail_at(f, manifest.ENV_NAME, "cannot be removed");
            return FAILED;
        };
    }
    fault.status(f, text.concat("Resolved packages: ", text.from_int(list.len(lock.packages))));
    answer(f, row2("resolved", text.from_int(list.len(lock.packages))));
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
    const here = project_dir(f) orelse return FAILED;
    const net = plan.network();
    var entries: list.List[manifest.Installed] = list.new();
    const total = list.len(lock.packages);
    var done = 0;
    var installed = 0;
    fault.status(f, text.concat("Installing dependencies: ", text.from_int(total)));
    for (list.to_array(lock.packages)) |p| {
        fault.status(f, progress.package(done, total, "Checking", p.name));
        const digest = digest_of(p.tree);
        if (store.check(h, digest) != store.READY) {
            fault.status(f, progress.package(done, total, "Installing", p.name));
            const dir = plan.source_dir(f, net, p.source) orelse return FAILED;
            fault.status(f, progress.package(done, total, "Copying and verifying", p.name));
            const got = store.install(f, h, dir);
            if (!f.ok) { return FAILED; }
            if (!text.eq(got, digest)) {
                // A path or git source that no longer hashes to what the
                // lockfile says has been *edited*, and resolving again records
                // what it is now. A registry source cannot have been: a
                // published version is never changed, which is the rule the
                // whole scheme rests on. So the same mismatch means something
                // different -- the index promised a tree the revision does not
                // hold -- and telling somebody to resolve again would send them
                // round a loop that records the same promise every time.
                if (text.starts_with(p.source, "reg+")) {
                    fault.fail(f, text.concat(text.concat("`", p.name),
                        text.concat("` hashes to sha256:", text.concat(got,
                            text.concat(", and the registry promised ", p.tree)))));
                    return BROKEN;
                }
                fault.fail(f, text.concat(text.concat("`", p.name),
                    "` has changed since it was resolved -- run `ingot resolve`"));
                return NEEDS_RESOLVING;
            }
            answer(f, row3("installed", p.name, digest));
            installed += 1;
        } else {
            fault.status(f, progress.package(done, total, "Cached", p.name));
        }
        // Built for every package rather than only the ones that had to be
        // fetched: `ingot.env` describes the whole project, and a second
        // `install` that had nothing to do must still leave one behind.
        const entry = store.entry(h, digest);
        list.push(entries, manifest.Installed{
            .name = p.name,
            .dir = entry,
            .root = facade(f, entry, p.name),
            .deps = p.deps,
        });
        if (!f.ok) { return FAILED; }
        done += 1;
        fault.status(f, progress.package(done, total, "Ready", p.name));
    }

    const root = manifest.Installed{
        .name = m.name,
        .dir = here,
        .root = m.root,
        .deps = dep_names(m),
    };
    fault.status(f, "Writing ingot.env");
    io.write_file(manifest.ENV_NAME, manifest.write_env(root, entries)) catch {
        fault.fail_at(f, manifest.ENV_NAME, "cannot be written");
        return FAILED;
    };
    // The absolute path, not `ingot.lock`. `env/` names a lockfile by the hash
    // of its path, so a relative one makes every project on the machine the
    // same environment -- and then one project's `install` unregisters
    // another's, whose entries the next `gc` deletes.
    store.register(f, h, path.join(here, lock_path()));
    if (!f.ok) { return FAILED; }
    fault.status(f, text.join([]str{ text.from_int(total), " packages ready (",
        text.from_int(installed), " installed, ", text.from_int(total - installed), " cached)" }, ""));
    answer(f, row2("ready", text.from_int(list.len(lock.packages))));
    return OK;
}

/// Where this process is, absolutely and spelled with `/`.
///
/// `-C` has already moved us, so the working directory *is* the project. The
/// answer has to be absolute twice over: `ingot.env` is read by a compiler that
/// may be run from anywhere, and an environment is named by its lockfile's path.
fn project_dir(f: fault.Fault) ?str {
    const d = os.cwd() catch {
        fault.fail(f, "cannot work out which directory this is");
        return null;
    };
    return path.normalise(d);
}

/// Which file in a store entry an `@import` of that package resolves to.
///
/// The package's own manifest says so, and a package that was resolved has one.
/// The fallback is written out rather than asserted because a store entry is
/// content-addressed and its `ingot.toml` is a file in it like any other: a
/// tree that has lost one is a tree, and `default_root` is what `init` would
/// have written anyway.
fn facade(f: fault.Fault, entry: str, name: str) str {
    const file = path.join(entry, manifest.MANIFEST_NAME);
    if (!io.exists(file)) { return default_root(name); }
    const m = manifest.read(f, file) orelse {
        f.ok = true;
        return default_root(name);
    };
    if (text.len(m.root) == 0) { return default_root(name); }
    return m.root;
}

/// What the root package may import: the names its manifest asked for.
fn dep_names(m: manifest.Manifest) []str {
    var out = []str{};
    for (list.to_array(m.deps)) |d| { out = array.push(out, d.name); }
    return out;
}

/// Ready, or the smallest thing that has to happen first.
///
/// The exit status is the answer, so a script can ask without reading a word
/// of this. Every check is done rather than the first failure returned, because
/// "resolve, then install" is more useful to be told at once.
fn verify(f: fault.Fault) i64 {
    const m = read_here(f) orelse return BROKEN;
    if (!io.exists(lock_path())) {
        answer(f, row2("needs", "resolve"));
        return NEEDS_RESOLVING;
    }
    const lock = read_lock_here(f) orelse return BROKEN;
    if (!text.eq(lock.manifest, m.digest)) {
        answer(f, row3("stale", lock_path(), "ingot.toml has changed"));
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
            answer(f, row3("damaged", p.name, digest));
            worst = BROKEN;
            continue;
        }
        if (state == store.MISSING) {
            answer(f, row3("missing", p.name, digest));
            if (worst < NEEDS_INSTALLING) { worst = NEEDS_INSTALLING; }
            continue;
        }
        // A path dependency that has been edited since it was resolved is
        // neither missing nor damaged: the store holds exactly what it was
        // told to. It is the *lockfile* that is out of date.
        if (local_source(p)) |dir| {
            if (fs.is_dir(dir)) {
                const now = store.tree_hash(f, dir);
                if (f.ok and !text.eq(now, digest)) {
                    answer(f, row3("changed", p.name, dir));
                    if (worst < NEEDS_RESOLVING) { worst = NEEDS_RESOLVING; }
                }
                f.ok = true;
            }
        }
    }
    // A store that satisfies the lockfile is still not a project that can be
    // compiled: the loader finds a package's files through `ingot.env`, and
    // only `install` writes one. Asked last and only when nothing else is
    // wrong, because a project that is missing entries needs `install` anyway
    // and has already been told so.
    if (worst == OK and !io.exists(manifest.ENV_NAME)) {
        answer(f, row2("needs", "install"));
        worst = NEEDS_INSTALLING;
    }
    if (worst == OK) { answer(f, row2("ready", text.from_int(list.len(lock.packages)))); }
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

/// Where a locked package's source is *without fetching anything*.
///
/// `verify` uses this and `install` does not: asking whether a project is
/// ready must not go to the network, and making it ready must be allowed to.
fn local_source(p: manifest.Locked) ?str {
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
        answer(f, row4(p.name, p.version, p.source, p.tree));
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
        answer(f, row2("nothing", want));
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
    // A registry index is an ordinary store entry reached by a pointer file
    // rather than by a lockfile, so without this the first collection after an
    // update deletes the registry and the next resolve fetches it again.
    for (store.held_indexes(h)) |digest| { list.push(keep, digest); }
    const removed = store.collect(f, h, keep);
    if (!f.ok) { return FAILED; }
    answer(f, row2("removed", text.from_int(removed)));
    answer(f, row2("kept", text.from_int(list.len(keep))));
    return OK;
}

fn show_store(f: fault.Fault) i64 {
    const h = store.home() catch {
        fault.fail(f, "cannot work out where the store is");
        return FAILED;
    };
    answer(f, row2("home", h));
    answer(f, row2("objects", store.objects(h)));
    var entries = 0;
    if (fs.is_dir(store.objects(h))) {
        entries = array.len(fs.read_dir(store.objects(h)) catch []str{});
    }
    answer(f, row2("entries", text.from_int(entries)));
    answer(f, row2("environments", text.from_int(array.len(store.registered(h)))));
    return OK;
}

// ---------------------------------------------------------------------------
// Tab-separated output
// ---------------------------------------------------------------------------

fn row2(a: str, b: str) str { return text.concat(a, text.concat("\t", b)); }

/// Complete transient stderr output before a TSV row reaches the terminal.
fn answer(f: fault.Fault, message: str) void {
    fault.flush(f);
    print(message);
    return;
}
fn row3(a: str, b: str, c: str) str { return row2(a, row2(b, c)); }
fn row4(a: str, b: str, c: str, d: str) str { return row2(a, row3(b, c, d)); }
