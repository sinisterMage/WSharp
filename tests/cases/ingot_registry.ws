// The package index: what a registry says, and what it refuses to say.
//
// A registry is a *directory*, which is what lets this be a case at all --
// building one under `os.temp_dir()` and reading it back exercises the same
// code the tool runs against the public one, with no network and no server.
// What a case cannot reach is the fetch that puts the directory there; that is
// `plan.pull`, which the git-dependency path already covers.
//
// The last third drives the solver through a registry-backed provider, which is
// the first time in this project that `versions()` answers with more than one
// version -- a path dependency offers exactly one, so until now the search had
// nothing to search.
// expect: acme/json is a name
// expect: json is not
// expect: Acme/json is not
// expect: std/json is not
// expect: acme/js on is not
// expect: -acme/json is not
// expect: Foundry
// expect: 3
// expect: 1.0.0 1111111111111111111111111111111111111111 -
// expect: 1.2.0 2222222222222222222222222222222222222222 acme/http >=1.0.0 <2.0.0
// expect: 2.0.0 3333333333333333333333333333333333333333 yanked
// expect: newest 1.2.0
// expect: 2.0.0 is still readable
// expect: acme/http
// expect: acme/json
// expect: nothing is not here
// expect: acme/json 1.2.0
// expect: acme/http 1.1.0
// expect: versions.toml: 1.2.0 is written twice
// expect: versions.toml: 1.0.0 needs a `rev` that is a full object id
// expect: package.toml: `repo` must be an `https://` URL
// expect: Registry.toml: was written by a newer ingot than this one
const array = @import("std/array");
const bytes = @import("std/bytes");
const crypto = @import("std/crypto");
const fault = @import("ingot/fault");
const fs = @import("std/fs");
const io = @import("std/io");
const list = @import("std/list");
const os = @import("std/os");
const path = @import("std/path");
const pubgrub = @import("ingot/pubgrub");
const registry = @import("ingot/registry");
const semver = @import("ingot/semver");
const text = @import("std/str");

const HEX40 = "1111111111111111111111111111111111111111";
const HEX64 = "1111111111111111111111111111111111111111111111111111111111111111";

fn name_rules() void {
    say("acme/json", registry.valid_name("acme/json"));
    // One segment is a landgrab, an upper-case letter is two names on a
    // case-insensitive filesystem, and `std` is a namespace the compiler
    // resolves before it ever looks at a package.
    say("json", registry.valid_name("json"));
    say("Acme/json", registry.valid_name("Acme/json"));
    say("std/json", registry.valid_name("std/json"));
    say("acme/js on", registry.valid_name("acme/js on"));
    say("-acme/json", registry.valid_name("-acme/json"));
    return;
}

fn say(name: str, ok: bool) void {
    if (ok) { print(text.concat(name, " is a name")); return; }
    print(text.concat(name, " is not"));
    return;
}

/// Write a registry with two packages and three versions of one of them.
fn build(root: str) !void {
    try fs.mkdir_all(path.join(root, "packages/acme/json"));
    try fs.mkdir_all(path.join(root, "packages/acme/http"));
    try io.write_file(path.join(root, "Registry.toml"),
        "[registry]\nversion = 1\nname = \"Foundry\"\n");

    try io.write_file(path.join(root, "packages/acme/json/package.toml"),
        "[package]\nname = \"acme/json\"\nrepo = \"https://example.invalid/json.git\"\n");
    try io.write_file(path.join(root, "packages/acme/json/versions.toml"),
        text.concat(text.concat(
            "[[version]]\nversion = \"1.0.0\"\nrev = \"1111111111111111111111111111111111111111\"\ntree = \"sha256:1111111111111111111111111111111111111111111111111111111111111111\"\n\n",
            "[[version]]\nversion = \"1.2.0\"\nrev = \"2222222222222222222222222222222222222222\"\ntree = \"sha256:2222222222222222222222222222222222222222222222222222222222222222\"\n\n[version.dependencies]\n\"acme/http\" = \"^1.0.0\"\n\n"),
            "[[version]]\nversion = \"2.0.0\"\nrev = \"3333333333333333333333333333333333333333\"\ntree = \"sha256:3333333333333333333333333333333333333333333333333333333333333333\"\nyanked = true\n"));

    try io.write_file(path.join(root, "packages/acme/http/package.toml"),
        "[package]\nname = \"acme/http\"\nrepo = \"https://example.invalid/http.git\"\n");
    try io.write_file(path.join(root, "packages/acme/http/versions.toml"),
        "[[version]]\nversion = \"1.1.0\"\nrev = \"4444444444444444444444444444444444444444\"\ntree = \"sha256:4444444444444444444444444444444444444444444444444444444444444444\"\n");
    return;
}

fn read_it(f: fault.Fault, root: str) bool {
    const ix = registry.open(f, root) orelse return false;
    print(ix.name);

    const p = registry.package(f, ix, "acme/json") orelse return false;
    print_int(list.len(p.releases));
    for (list.to_array(p.releases)) |r| {
        // Version, revision, and then what it needs -- a yanked release says so
        // instead, because that is the fact about it that matters.
        var tail = "-";
        if (r.yanked) { tail = "yanked"; }
        for (list.to_array(r.needs)) |need| {
            tail = text.concat(need.package, text.concat(" ", semver.show(need.range)));
        }
        print(text.concat(semver.render(r.version), text.concat(" ",
            text.concat(r.rev, text.concat(" ", tail)))));
    }

    // `newest` skips the yanked 2.0.0, and `release` still finds it: yanking
    // withdraws a version from being *chosen*, and a lockfile that already
    // names one has to keep installing.
    const best = registry.newest(p) orelse return false;
    print(text.concat("newest ", semver.render(best)));
    const two = semver.parse("2.0.0") orelse return false;
    if (registry.release(p, two)) |r| { print("2.0.0 is still readable"); }

    // A directory holding a `package.toml` is a package; that rule is why the
    // registry needs no central list of its contents.
    //
    // In byte order, which is asserted rather than incidental: `fs.read_dir`
    // answers in the filesystem's order, and this case listed `acme/json`
    // before `acme/http` on APFS and the other way round on ext4 until `names`
    // sorted for itself.
    for (registry.names(ix)) |n| { print(n); }

    // A package this registry does not hold is null with *nothing said*: which
    // package was wanted, and what asked for it, is known one level up.
    if (registry.package(f, ix, "acme/nothing")) |q| {
        print("found something that is not there");
        return false;
    }
    if (!f.ok) { return false; }
    print("nothing is not here");
    return true;
}

/// Each way an index can be wrong, and the file it is wrong in.
fn refusals(root: str) !void {
    const dir = path.join(root, "packages/acme/json");

    try io.write_file(path.join(dir, "versions.toml"),
        text.concat("[[version]]\nversion = \"1.2.0\"\nrev = \"2222222222222222222222222222222222222222\"\ntree = \"sha256:2222222222222222222222222222222222222222222222222222222222222222\"\n\n",
                    "[[version]]\nversion = \"1.2.0\"\nrev = \"2222222222222222222222222222222222222222\"\ntree = \"sha256:2222222222222222222222222222222222222222222222222222222222222222\"\n"));
    refused(root, "acme/json");

    // A short revision is the classic one: it looks like an object id and names
    // nothing a fetch can ask for.
    try io.write_file(path.join(dir, "versions.toml"),
        "[[version]]\nversion = \"1.0.0\"\nrev = \"2222\"\ntree = \"sha256:1111111111111111111111111111111111111111111111111111111111111111\"\n");
    refused(root, "acme/json");

    try io.write_file(path.join(dir, "package.toml"),
        "[package]\nname = \"acme/json\"\nrepo = \"git://example.invalid/json.git\"\n");
    refused(root, "acme/json");

    // A registry from a newer ingot is refused rather than guessed at, which is
    // the part `manifest.LOCK_VERSION` plays for a lockfile.
    try io.write_file(path.join(root, "Registry.toml"),
        "[registry]\nversion = 99\nname = \"Foundry\"\n");
    const f = fault.none();
    if (registry.open(f, root)) |ix| { print("opened a registry from the future"); }
    print(tail(f.message));
    return;
}

fn refused(root: str, name: str) void {
    const f = fault.none();
    const ix = registry.open(f, root) orelse { print(f.message); return; };
    if (registry.package(f, ix, name)) |p| { print("read something broken"); return; }
    print(tail(f.message));
    return;
}

/// A `<file>: <message>` with the temporary directory's random name cut off.
///
/// Split at the first `": "` and take the basename of only the left half. The
/// *message* may hold a `/` of its own -- ``must be an `https://` URL`` does --
/// so taking the last path component of the whole string turns it into "` URL".
fn tail(message: str) str {
    const at = text.find(message, ": ");
    if (at < 0) { return message; }
    const parts = text.split(text.substr(message, 0, at), "/");
    const n = array.len(parts);
    if (n < 1) { return message; }
    return text.concat(parts[n - 1], text.substr(message, at, text.len(message)));
}

/// The solver, over the index.
///
/// The provider stays total -- `versions` and `dependencies` cannot fail -- by
/// reading the reachable subgraph first, which is exactly what `plan.discover`
/// does with the real one.
fn solve_it(f: fault.Fault, root: str) bool {
    const ix = registry.open(f, root) orelse return false;
    var packages: list.List[registry.Package] = list.new();
    const json = registry.package(f, ix, "acme/json") orelse return false;
    const http = registry.package(f, ix, "acme/http") orelse return false;
    list.push(packages, json);
    list.push(packages, http);

    const provider = pubgrub.Provider{
        .versions = fn (p: str) list.List[semver.Version] {
            var out: list.List[semver.Version] = list.new();
            for (list.to_array(packages)) |entry| {
                if (!text.eq(entry.name, p)) { continue; }
                for (list.to_array(entry.releases)) |r| {
                    if (r.yanked) { continue; }
                    list.push(out, r.version);
                }
            }
            return out;
        },
        .dependencies = fn (p: str, v: semver.Version) list.List[pubgrub.Need] {
            for (list.to_array(packages)) |entry| {
                if (!text.eq(entry.name, p)) { continue; }
                for (list.to_array(entry.releases)) |r| {
                    if (semver.eq(r.version, v)) { return r.needs; }
                }
            }
            var none: list.List[pubgrub.Need] = list.new();
            return none;
        },
    };

    // A root that wants `acme/json ^1.0.0`: 2.0.0 is yanked and out of range
    // anyway, so 1.2.0 wins and drags `acme/http` in behind it.
    var needs: list.List[pubgrub.Need] = list.new();
    list.push(needs, pubgrub.Need{
        .package = "acme/json",
        .range = semver.requirement("^1.0.0") orelse semver.any(),
    });
    const root_version = semver.parse("0.1.0") orelse return false;
    const with_root = pubgrub.Provider{
        .versions = fn (p: str) list.List[semver.Version] {
            if (text.eq(p, "app")) {
                var one: list.List[semver.Version] = list.new();
                list.push(one, root_version);
                return one;
            }
            return (provider.versions)(p);
        },
        .dependencies = fn (p: str, v: semver.Version) list.List[pubgrub.Need] {
            if (text.eq(p, "app")) { return needs; }
            return (provider.dependencies)(p, v);
        },
    };

    const answer = pubgrub.solve(with_root, "app", root_version);
    if (!answer.ok) { print(answer.report); return false; }
    const n = list.len(answer.names);
    var i = 0;
    while (i < n) : (i += 1) {
        print(text.concat(list.get(answer.names, i),
            text.concat(" ", semver.render(list.get(answer.versions, i)))));
    }
    return true;
}

fn run(f: fault.Fault, root: str) !i64 {
    name_rules();
    try build(root);
    if (!read_it(f, root)) { return 1; }
    if (!solve_it(f, root)) { return 1; }
    // Last, because each one leaves the registry broken behind it.
    try refusals(root);
    return 0;
}

fn main() i64 {
    const f = fault.none();
    // Under the system's own temporary directory with a random name, because
    // this case runs twice -- once plainly and once under `--gc-stress`.
    const suffix = bytes.to_hex(crypto.random(6) catch return 2);
    const root = path.join(os.temp_dir(), text.concat("ingot-registry-", suffix));
    const code = run(f, root) catch 1;
    if (!f.ok) { print(f.message); }
    fs.remove_tree(root) catch print("could not clean up");
    return code;
}
