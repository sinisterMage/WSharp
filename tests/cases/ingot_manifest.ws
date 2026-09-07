// `ingot.toml` and `ingot.lock`: read, written, and read back. And `ingot.env`,
// which is written and never read here.
//
// The first two go through `std/toml` in both directions. A lockfile written by
// a hand-rolled emitter and read by a real parser is a bug that waits for the
// one entry with a quotation mark in it, so the round trip is the test.
//
// The third has no round trip because it has no W# reader: the compiler's
// loader is what reads it, and that is Rust. So what is checked here is the
// shape the loader was written against, byte for byte.
// expect: acme/json
// expect: 1.2.0
// expect: src/json.ws
// expect: 3
// expect: acme/json
// expect: 3
// expect: acme/http	^1.0.0	-	-
// expect: util	-	../util	-
// expect: parse	-	-	8f2c
// expect: digest
// expect: no package table: ingot.toml: there is no `[package]` table
// expect: no name: ingot.toml: `[package]` has no `name`
// expect: no version: ingot.toml: `[package]` has no `version`
// expect: two sources: ingot.toml: dependency `b` needs exactly one of `version`, `path` and `git`
// expect: no source: ingot.toml: dependency `b` needs exactly one of `version`, `path` and `git`
// expect: git with no rev: ingot.toml: dependency `b` is a git dependency and needs a `rev`
// expect: not toml: ingot.toml: line 1: expected `]` to close a table header
// expect: version = 1
// expect: manifest = "cafe"
// expect: 
// expect: [[package]]
// expect: name = "util"
// expect: version = "0.3.0"
// expect: source = "path+../util"
// expect: tree = "sha256:abcd"
// expect: dependencies = ["core"]
// expect: 
// expect: util	0.3.0	path+../util	sha256:abcd
// expect: core
// expect: ingot.lock: was written by a newer ingot than this one
// expect: myapp	/work/app	src/myapp.ws	util	acme/json
// expect: util	/store/c14b	src/util.ws
// expect:
const fault = @import("ingot/fault");
const list = @import("std/list");
const manifest = @import("ingot/manifest");
const text = @import("std/str");

fn read(name: str, src: str) ?manifest.Manifest {
    const f = fault.none();
    const m = manifest.of_text(f, "ingot.toml", src, ".") orelse {
        print(text.concat(name, text.concat(": ", f.message)));
        return null;
    };
    return m;
}

fn or_dash(s: str) str {
    if (text.len(s) == 0) { return "-"; }
    return s;
}

fn main() i64 {
    const src = "[package]\nname = \"acme/json\"\nversion = \"1.2.0\"\n\n[dependencies]\n\"acme/http\" = \"^1.0.0\"\nutil = { path = \"../util\" }\nparse = { git = \"https://example.invalid/parse.git\", rev = \"8f2c\" }\n";
    const m = read("manifest", src) orelse return 1;
    print(m.name);
    print(m.version);
    // `acme/json` is `src/json.ws` unless the manifest says otherwise, so the
    // common package needs no `root` line.
    print(m.root);
    print_int(list.len(m.deps));

    // Writing it out and reading it back must give the same thing, including
    // the three spellings a dependency has.
    const again = read("round trip", manifest.write(m)) orelse return 1;
    print(again.name);
    print_int(list.len(again.deps));
    for (list.to_array(again.deps)) |d| {
        // `-` for a field that is not set, because the case harness trims each
        // expected line and a trailing tab cannot be written in one.
        print(text.concat(d.name, text.concat("\t", text.concat(or_dash(d.req),
            text.concat("\t", text.concat(or_dash(d.dir), text.concat("\t", or_dash(d.rev))))))));
    }
    // The digest is the manifest's bytes, which is what the lockfile records so
    // that `verify` can tell a stale lock from a current one -- a timestamp
    // could not, because a checkout does not preserve one.
    print(if (text.eq(m.digest, manifest.digest_of(src))) "digest" else "DIGEST MISMATCH");

    // The refusals.
    read("no package table", "x = 1\n");
    read("no name", "[package]\nversion = \"1\"\n");
    read("no version", "[package]\nname = \"a\"\n");
    read("two sources", "[package]\nname = \"a\"\nversion = \"1\"\n[dependencies]\nb = { path = \"x\", git = \"y\" }\n");
    read("no source", "[package]\nname = \"a\"\nversion = \"1\"\n[dependencies]\nb = {}\n");
    read("git with no rev", "[package]\nname = \"a\"\nversion = \"1\"\n[dependencies]\nb = { git = \"y\" }\n");
    read("not toml", "[package\n");

    // A lockfile, round-tripped the same way.
    var packages: list.List[manifest.Locked] = list.new();
    list.push(packages, manifest.Locked{
        .name = "util",
        .version = "0.3.0",
        .source = "path+../util",
        .tree = "sha256:abcd",
        .deps = []str{ "core" },
    });
    const lock = manifest.Lock{ .version = manifest.LOCK_VERSION, .manifest = "cafe", .packages = packages };
    const written = manifest.write_lock(lock);
    print(written);
    const g = fault.none();
    const back = manifest.lock_of_text(g, "ingot.lock", written) orelse {
        print(g.message);
        return 1;
    };
    const one = manifest.locked(back, "util") orelse {
        print("util is missing from the lockfile it was just written to");
        return 1;
    };
    print(text.concat(one.name, text.concat("\t", text.concat(one.version,
        text.concat("\t", text.concat(one.source, text.concat("\t", one.tree)))))));
    print(one.deps[0]);

    // A lockfile from a newer ingot is refused rather than guessed at.
    const h = fault.none();
    const newer = manifest.lock_of_text(h, "ingot.lock", "version = 99\n") orelse {
        print(h.message);
        return env();
    };
    return 0;
}

/// `ingot.env`, which the compiler's loader reads and nothing here does.
///
/// Dependencies are the fourth field onwards rather than a list inside one, so
/// the file has exactly one separator and a package name is whatever a name is.
/// A package with none simply stops after three fields -- there is no empty
/// field to leave, which is also what keeps a line trimmable by the harness.
fn env() i64 {
    const root = manifest.Installed{
        .name = "myapp",
        .dir = "/work/app",
        .root = "src/myapp.ws",
        .deps = []str{ "util", "acme/json" },
    };
    var packages: list.List[manifest.Installed] = list.new();
    list.push(packages, manifest.Installed{
        .name = "util",
        .dir = "/store/c14b",
        .root = "src/util.ws",
        .deps = []str{},
    });
    print(manifest.write_env(root, packages));
    return 0;
}
