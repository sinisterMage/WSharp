//! The verbs, end to end, against the built binary.
//!
//! A W# case (`tests/cases/ingot_*.ws`) can reach the store and the manifest
//! reader directly, and does. What it cannot reach is the tool: a verb's job is
//! to read a directory, write two files and answer with an exit status, and the
//! exit status is the interface `verify` was designed around. So this drives
//! `ingot` as a subprocess, exactly as a script would.
//!
//! Every project is built under a directory of its own with random bytes in its
//! name, and `WSHARP_HOME` points at a store inside it -- so the tests neither
//! see each other nor touch the developer's real store.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// What `ingot verify` answers with. Spelled out rather than imported: the test
/// should see what a script sees.
const READY: i32 = 0;
const NEEDS_INSTALLING: i32 = 1;
const NEEDS_RESOLVING: i32 = 2;
const BROKEN: i32 = 3;
const FAILED: i32 = 4;

struct Project {
    root: PathBuf,
}

impl Project {
    fn new(name: &str) -> Project {
        let mut root = std::env::temp_dir();
        root.push(format!(
            "ingot-verbs-{}-{name}-{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("a directory to work in");
        Project { root }
    }

    fn store(&self) -> PathBuf {
        self.root.join("home")
    }

    fn dir(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    /// Write a package with a manifest and one source file.
    fn package(&self, name: &str, version: &str, deps: &str, body: &str) -> PathBuf {
        let dir = self.dir(name);
        std::fs::create_dir_all(dir.join("src")).expect("a package directory");
        std::fs::write(
            dir.join("ingot.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"{version}\"\n\n[dependencies]\n{deps}"
            ),
        )
        .expect("a manifest");
        std::fs::write(dir.join("src").join(format!("{name}.ws")), body).expect("a source file");
        dir
    }

    fn run(&self, dir: &Path, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ingot"));
        command
            .env("WSHARP_HOME", self.store())
            .arg("-C")
            .arg(dir)
            .args(args);
        command.output().expect("ingot runs")
    }

    /// Run a verb and insist on its exit status, returning stdout.
    fn expect(&self, dir: &Path, args: &[&str], status: i32) -> String {
        let out = self.run(dir, args);
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let code = out.status.code().unwrap_or(-1);
        assert_eq!(
            code,
            status,
            "`ingot {}` answered {code} rather than {status}\nstdout:\n{stdout}stderr:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
        stdout
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// One field of a tab-separated line.
fn field(stdout: &str, row: usize, column: usize) -> String {
    stdout
        .lines()
        .nth(row)
        .unwrap_or_else(|| panic!("no line {row} in:\n{stdout}"))
        .split('\t')
        .nth(column)
        .unwrap_or_else(|| panic!("no column {column} in line {row} of:\n{stdout}"))
        .to_string()
}

/// The path a project takes on an ordinary day, in the order it takes it.
///
/// One test rather than six, because what is interesting is that the verbs
/// compose: `resolve` has to see what `add` wrote, `install` has to see what
/// `resolve` wrote, and `verify` has to agree with both.
#[test]
fn a_project_is_started_resolved_installed_and_verified() {
    let project = Project::new("happy");
    project.package("core", "0.1.0", "", "pub fn one() i64 { return 1; }\n");
    project.package(
        "util",
        "0.3.0",
        "core = { path = \"../core\" }\n",
        "pub fn twice(n: i64) i64 { return n * 2; }\n",
    );
    let app = project.dir("app");
    std::fs::create_dir_all(&app).expect("an app directory");

    project.expect(&app, &["init", "myapp"], READY);
    assert!(app.join("ingot.toml").exists(), "`init` writes a manifest");

    project.expect(&app, &["add", "util", "--path", "../util"], READY);

    // Nothing has been resolved, so there is nothing to install and nothing to
    // check: the smallest thing that has to happen first is `resolve`.
    project.expect(&app, &["verify"], NEEDS_RESOLVING);

    let resolved = project.expect(&app, &["resolve"], READY);
    assert_eq!(field(&resolved, 0, 1), "2", "util, and core behind it");
    assert!(
        app.join("ingot.lock").exists(),
        "`resolve` writes a lockfile"
    );

    // Resolved but not installed is its own answer, and a different one.
    project.expect(&app, &["verify"], NEEDS_INSTALLING);

    project.expect(&app, &["install"], READY);
    project.expect(&app, &["verify"], READY);

    let listed = project.expect(&app, &["list"], READY);
    assert_eq!(field(&listed, 0, 0), "util");
    assert_eq!(field(&listed, 1, 0), "core");
    assert!(field(&listed, 1, 3).starts_with("sha256:"));

    // The question a lockfile never answers on its own.
    let why = project.expect(&app, &["why", "core"], READY);
    assert_eq!(why.trim_end(), "myapp\tutil\tcore");

    let store = project.expect(&app, &["store"], READY);
    assert_eq!(field(&store, 2, 1), "2", "two trees are in the store");
}

/// A path dependency edited after it was resolved is neither missing nor
/// damaged: the store holds exactly what it was told to, and it is the lockfile
/// that is out of date. Telling those apart is what `verify` is for.
#[test]
fn editing_a_dependency_asks_for_a_new_resolution() {
    let project = Project::new("edited");
    let util = project.package(
        "util",
        "0.3.0",
        "",
        "pub fn twice(n: i64) i64 { return n * 2; }\n",
    );
    let app = project.dir("app");
    std::fs::create_dir_all(&app).expect("an app directory");
    project.expect(&app, &["init", "myapp"], READY);
    project.expect(&app, &["add", "util", "--path", "../util"], READY);
    project.expect(&app, &["resolve"], READY);
    project.expect(&app, &["install"], READY);
    project.expect(&app, &["verify"], READY);

    std::fs::write(
        util.join("src").join("util.ws"),
        "pub fn twice(n: i64) i64 { return n + n; }\n",
    )
    .expect("an edit");

    let out = project.expect(&app, &["verify"], NEEDS_RESOLVING);
    assert_eq!(field(&out, 0, 0), "changed");

    project.expect(&app, &["resolve"], READY);
    project.expect(&app, &["install"], READY);
    project.expect(&app, &["verify"], READY);
}

/// An entry that is there and is not what its name says is `damaged`, and
/// `install` repairs it rather than reporting success over it.
#[test]
fn a_damaged_entry_is_told_from_a_missing_one_and_is_repaired() {
    let project = Project::new("damaged");
    project.package(
        "util",
        "0.3.0",
        "",
        "pub fn twice(n: i64) i64 { return n * 2; }\n",
    );
    let app = project.dir("app");
    std::fs::create_dir_all(&app).expect("an app directory");
    project.expect(&app, &["init", "myapp"], READY);
    project.expect(&app, &["add", "util", "--path", "../util"], READY);
    project.expect(&app, &["resolve"], READY);
    project.expect(&app, &["install"], READY);

    let listed = project.expect(&app, &["list"], READY);
    let digest = field(&listed, 0, 3)
        .strip_prefix("sha256:")
        .expect("a store key")
        .to_string();
    let entry = project.store().join("store").join("sha256").join(&digest);
    std::fs::write(entry.join("src").join("util.ws"), "tampered\n").expect("tampering");

    let out = project.expect(&app, &["verify"], BROKEN);
    assert_eq!(field(&out, 0, 0), "damaged");

    project.expect(&app, &["install"], READY);
    project.expect(&app, &["verify"], READY);
}

/// Changing the manifest makes the lockfile stale, and `install` refuses rather
/// than working from it.
#[test]
fn a_changed_manifest_makes_the_lockfile_stale() {
    let project = Project::new("stale");
    project.package(
        "util",
        "0.3.0",
        "",
        "pub fn twice(n: i64) i64 { return n * 2; }\n",
    );
    project.package("other", "0.1.0", "", "pub fn nothing() i64 { return 0; }\n");
    let app = project.dir("app");
    std::fs::create_dir_all(&app).expect("an app directory");
    project.expect(&app, &["init", "myapp"], READY);
    project.expect(&app, &["add", "util", "--path", "../util"], READY);
    project.expect(&app, &["resolve"], READY);
    project.expect(&app, &["install"], READY);

    project.expect(&app, &["add", "other", "--path", "../other"], READY);
    let out = project.expect(&app, &["verify"], NEEDS_RESOLVING);
    assert_eq!(field(&out, 0, 0), "stale");
    project.expect(&app, &["install"], NEEDS_RESOLVING);

    project.expect(&app, &["resolve"], READY);
    project.expect(&app, &["install"], READY);
    project.expect(&app, &["verify"], READY);

    // And taking one out again is the same story backwards.
    project.expect(&app, &["remove", "other"], READY);
    project.expect(&app, &["verify"], NEEDS_RESOLVING);
}

/// What no registered lockfile reaches goes, and what one does reach stays.
#[test]
fn collecting_keeps_what_an_environment_reaches() {
    let project = Project::new("gc");
    let util = project.package(
        "util",
        "0.3.0",
        "",
        "pub fn twice(n: i64) i64 { return n * 2; }\n",
    );
    let app = project.dir("app");
    std::fs::create_dir_all(&app).expect("an app directory");
    project.expect(&app, &["init", "myapp"], READY);
    project.expect(&app, &["add", "util", "--path", "../util"], READY);
    project.expect(&app, &["resolve"], READY);
    project.expect(&app, &["install"], READY);

    // A second version of the same tree leaves the first unreachable.
    std::fs::write(
        util.join("src").join("util.ws"),
        "pub fn twice(n: i64) i64 { return n + n; }\n",
    )
    .expect("an edit");
    project.expect(&app, &["resolve"], READY);
    project.expect(&app, &["install"], READY);

    let before = project.expect(&app, &["store"], READY);
    assert_eq!(field(&before, 2, 1), "2", "both trees are in the store");

    let collected = project.expect(&app, &["gc"], READY);
    assert_eq!(field(&collected, 0, 1), "1", "the old tree goes");
    assert_eq!(field(&collected, 1, 1), "1", "the current one stays");

    let after = project.expect(&app, &["store"], READY);
    assert_eq!(field(&after, 2, 1), "1");
    project.expect(&app, &["verify"], READY);
}

/// The refusals, each of which says what to do.
#[test]
fn a_verb_that_cannot_be_carried_out_says_why() {
    let project = Project::new("refusals");
    project.package("core", "0.1.0", "", "pub fn one() i64 { return 1; }\n");
    let app = project.dir("app");
    std::fs::create_dir_all(&app).expect("an app directory");

    // Nothing here yet.
    let out = project.run(&app, &["verify"]);
    assert_eq!(out.status.code(), Some(BROKEN));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("ingot init"),
        "the message says what to do"
    );

    project.expect(&app, &["init", "myapp"], READY);
    project.expect(&app, &["init"], FAILED);

    // A version requirement needs a resolver, which is a later stage; saying
    // so beats "unsatisfiable".
    project.expect(&app, &["add", "acme/json", "1.0.0"], READY);
    let out = project.run(&app, &["resolve"]);
    assert_eq!(out.status.code(), Some(FAILED));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("cannot yet fetch"),
        "stdout was:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    project.expect(&app, &["remove", "acme/json"], READY);
    project.expect(&app, &["remove", "acme/json"], FAILED);

    // A path that points at a package with a different name in it.
    project.expect(&app, &["add", "wrong", "--path", "../core"], READY);
    let out = project.run(&app, &["resolve"]);
    assert_eq!(out.status.code(), Some(FAILED));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("whose package is called core"),
        "stdout was:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );

    project.expect(&app, &["nonsense"], FAILED);
}

/// `ingot run` is the one verb that is the compiler, and it passes the rest of
/// the command line through untouched.
#[test]
fn run_compiles_a_program_and_hands_it_the_arguments() {
    let project = Project::new("run");
    let file = project.root.join("hello.ws");
    std::fs::write(
        &file,
        "const array = @import(\"std/array\");\n\
         const os = @import(\"std/os\");\n\
         fn main() i64 {\n\
         \x20   for (os.args()) |a| { print(a); }\n\
         \x20   return array.len(os.args());\n\
         }\n",
    )
    .expect("a program");

    let out = project.run(&project.root, &["run", "hello.ws", "one", "--two"]);
    assert_eq!(out.status.code(), Some(2), "`main` returns the count");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "one\n--two\n",
        "and nothing was interpreted on the way through"
    );
}
