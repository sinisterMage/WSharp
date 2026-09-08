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
//!
//! # Where `ingot` comes from
//!
//! It is not a cargo binary any more. `ingot` is a W# program -- `ingot/main`,
//! compiled into `wsharp` along with the rest of the library -- and it is built
//! by `wsharp build --module ingot/main`, which is what [`ingot`] below does
//! once for the whole file. That is also how a release is made, so these tests
//! drive the same artefact a user gets rather than one made another way.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

/// The `ingot` binary, built from W# on first use.
///
/// Once per test *binary* rather than per test, because building it is a
/// compile and a link and every test here wants the same one.
fn ingot() -> &'static Path {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("ingot-build-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a directory to build in");
        let exe = dir.join(format!("ingot{}", std::env::consts::EXE_SUFFIX));
        let out = Command::new(env!("CARGO_BIN_EXE_wsharp"))
            .arg("build")
            .arg("--module")
            .arg("ingot/main")
            .arg("-o")
            .arg(&exe)
            .output()
            .expect("wsharp runs");
        assert!(
            out.status.success(),
            "could not build ingot from `ingot/main`:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        // Beside it, because `ingot run` looks for the compiler there first --
        // and because that is the layout a release has.
        let beside = dir.join(format!("wsharp{}", std::env::consts::EXE_SUFFIX));
        let _ = std::fs::remove_file(&beside);
        std::fs::copy(env!("CARGO_BIN_EXE_wsharp"), &beside).expect("a compiler beside it");
        exe
    })
}

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
        let project = Project { root };
        // An *empty* registry rather than none, because `INGOT_REGISTRY` naming
        // something that is not a directory is a URL, and a URL is fetched. A
        // test that meant "this package is published nowhere" would otherwise
        // go to the network to find that out.
        std::fs::create_dir_all(project.registry()).expect("a registry directory");
        std::fs::write(
            project.registry().join("Registry.toml"),
            "[registry]\nversion = 1\nname = \"Testing\"\n",
        )
        .expect("a Registry.toml");
        project
    }

    fn store(&self) -> PathBuf {
        self.root.join("home")
    }

    /// The registry every verb in this file is pointed at.
    ///
    /// **Set on every invocation, whether or not the test uses a registry.**
    /// `INGOT_REGISTRY` falls back to the public one, so a test that left it
    /// unset would resolve against the real Foundry over the real network --
    /// which is slow, needs a machine to be online, and makes what this suite
    /// asserts depend on what somebody published this morning. Pointing it at a
    /// directory that may not even exist is what keeps the suite hermetic: a
    /// registry is a directory, so this is not a mock.
    fn registry(&self) -> PathBuf {
        self.root.join("registry")
    }

    /// Write a registry holding one version of one package.
    ///
    /// `deps` is the body of the release's `[version.dependencies]`, so `""` is
    /// a package that needs nothing.
    fn publish(&self, name: &str, version: &str, repo: &str, rev: &str, tree: &str, deps: &str) {
        let dir = self.registry().join("packages").join(name);
        std::fs::create_dir_all(&dir).expect("a package directory");
        std::fs::write(
            dir.join("package.toml"),
            format!("[package]\nname = \"{name}\"\nrepo = \"{repo}\"\n"),
        )
        .expect("a package.toml");
        let versions = dir.join("versions.toml");
        let mut body = std::fs::read_to_string(&versions).unwrap_or_default();
        body.push_str(&format!(
            "\n[[version]]\nversion = \"{version}\"\nrev = \"{rev}\"\ntree = \"sha256:{tree}\"\n"
        ));
        if !deps.is_empty() {
            body.push_str(&format!("\n[version.dependencies]\n{deps}\n"));
        }
        std::fs::write(&versions, body).expect("a versions.toml");
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

    /// Another file in a package, beside its facade.
    ///
    /// A package of more than one file is the case the facade exists for, and
    /// the only way to reach this one from outside is through what the facade
    /// renames.
    fn beside(&self, package: &Path, file: &str, body: &str) {
        std::fs::write(package.join("src").join(file), body).expect("a source file");
    }

    /// Run the *compiler* rather than the tool, from outside the project.
    ///
    /// The point of the separation: `wsharp` needs no store, no network and no
    /// verb to build a project somebody else installed, because everything it
    /// needs is in the `ingot.env` beside the lockfile.
    ///
    fn compile(&self, file: &Path) -> Output {
        Command::new(env!("CARGO_BIN_EXE_wsharp"))
            .env("WSHARP_HOME", self.store())
            .arg("run")
            .arg(file)
            .output()
            .expect("wsharp runs")
    }

    fn run(&self, dir: &Path, args: &[&str]) -> Output {
        let mut command = Command::new(ingot());
        command
            .env("WSHARP_HOME", self.store())
            .env("INGOT_REGISTRY", self.registry())
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
        String::from_utf8_lossy(&out.stderr).contains("ingot init"),
        "the message says what to do, on the error stream:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    project.expect(&app, &["init", "myapp"], READY);
    project.expect(&app, &["init"], FAILED);

    // A version requirement on a package the registry does not hold names the
    // package and the registry, which beats the solver's "no versions match".
    project.expect(&app, &["add", "acme/json", "1.0.0"], READY);
    let out = project.run(&app, &["resolve"]);
    assert_eq!(out.status.code(), Some(FAILED));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("`acme/json` is not in Testing"),
        "stderr was:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    project.expect(&app, &["remove", "acme/json"], READY);
    project.expect(&app, &["remove", "acme/json"], FAILED);

    // A path that points at a package with a different name in it.
    project.expect(&app, &["add", "wrong", "--path", "../core"], READY);
    let out = project.run(&app, &["resolve"]);
    assert_eq!(out.status.code(), Some(FAILED));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("whose package is called core"),
        "stderr was:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    project.expect(&app, &["nonsense"], FAILED);
}

/// Resolution goes through PubGrub even when there is nothing to choose, which
/// is what makes a requirement on a path dependency *checked* rather than
/// ignored -- and what makes two packages that disagree about a third say so.
#[test]
fn a_requirement_that_cannot_be_met_is_explained() {
    let project = Project::new("conflict");
    project.package("core", "1.0.0", "", "pub fn one() i64 { return 1; }\n");
    project.package(
        "util",
        "0.3.0",
        "core = { path = \"../core\" }\n",
        "pub fn twice(n: i64) i64 { return n * 2; }\n",
    );
    let app = project.dir("app");
    std::fs::create_dir_all(&app).expect("an app directory");
    project.expect(&app, &["init", "myapp"], READY);
    project.expect(&app, &["add", "util", "--path", "../util"], READY);
    project.expect(&app, &["add", "core", "--path", "../core"], READY);
    project.expect(&app, &["resolve"], READY);

    // A package neither the graph nor the registry supplies is refused by name
    // rather than as the solver's honest but unhelpful "no versions match".
    project.expect(&app, &["add", "elsewhere", "^2.0.0"], READY);
    let out = project.run(&app, &["resolve"]);
    assert_eq!(out.status.code(), Some(FAILED));
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        stderr.contains("`elsewhere` is not in Testing"),
        "a dependency with no source says so:\n{stderr}"
    );
    project.expect(&app, &["remove", "elsewhere"], READY);
    project.expect(&app, &["resolve"], READY);

    // But a *version requirement* on a package a path dependency does supply
    // is checked, and failing it is a derivation rather than a shrug.
    std::fs::write(
        project.dir("util").join("ingot.toml"),
        "[package]\nname = \"util\"\nversion = \"0.3.0\"\n\n[dependencies]\ncore = \"^2.0.0\"\n",
    )
    .expect("an edit");
    let out = project.run(&app, &["resolve"]);
    assert_eq!(out.status.code(), Some(FAILED));
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        stderr.contains("no versions of core match >=2.0.0 <3.0.0")
            && stderr.contains("util 0.3.0 depends on core >=2.0.0 <3.0.0"),
        "the solver explains itself:\n{stderr}"
    );
}

/// A version dependency, resolved out of the registry.
///
/// **This stops at `resolve`, and that is the whole of what a registry adds to
/// it.** A release records the hash of its own tree, so choosing versions and
/// writing a lockfile fetches nothing at all -- which is what makes this
/// testable here. `install` is the verb that goes to the network, and it does
/// it through `plan.pull`, which the git-dependency path already exercises;
/// there is no git server in this suite to point a `repo` at.
#[test]
fn a_project_takes_a_dependency_from_the_registry() {
    let project = Project::new("registry");
    project.publish(
        "acme/json",
        "1.2.0",
        "https://example.invalid/json.git",
        "2222222222222222222222222222222222222222",
        "2222222222222222222222222222222222222222222222222222222222222222",
        "\"acme/http\" = \"^1.0.0\"\n",
    );
    // A newer version that is withdrawn, so `add` has something to skip.
    project.publish(
        "acme/json",
        "2.0.0",
        "https://example.invalid/json.git",
        "3333333333333333333333333333333333333333",
        "3333333333333333333333333333333333333333333333333333333333333333",
        "",
    );
    let versions = project.registry().join("packages/acme/json/versions.toml");
    let body = std::fs::read_to_string(&versions).expect("the versions file");
    std::fs::write(&versions, format!("{body}yanked = true\n")).expect("a yank");
    project.publish(
        "acme/http",
        "1.1.0",
        "https://example.invalid/http.git",
        "4444444444444444444444444444444444444444",
        "4444444444444444444444444444444444444444444444444444444444444444",
        "",
    );

    let app = project.dir("app");
    std::fs::create_dir_all(&app).expect("an app directory");
    project.expect(&app, &["init", "myapp"], READY);

    // `search` finds it, and reports the newest version worth having.
    let found = project.expect(&app, &["search", "json"], READY);
    assert_eq!(field(&found, 0, 0), "acme/json");
    assert_eq!(
        field(&found, 0, 1),
        "1.2.0",
        "the yanked 2.0.0 is not newest"
    );
    assert_eq!(field(&found, 1, 1), "1");

    // `add` with no version records a caret on the newest that is not yanked.
    project.expect(&app, &["add", "acme/json"], READY);
    let manifest = std::fs::read_to_string(app.join("ingot.toml")).expect("a manifest");
    assert!(
        manifest.contains("\"acme/json\" = \"^1.2.0\""),
        "add records the newest version:\n{manifest}"
    );

    // Resolving walks the index transitively and hashes nothing: the tree is
    // what the registry said it was.
    let resolved = project.expect(&app, &["resolve"], READY);
    assert_eq!(field(&resolved, 0, 1), "2", "the transitive dependency too");
    let listed = project.expect(&app, &["list"], READY);
    assert_eq!(field(&listed, 0, 0), "acme/json");
    assert_eq!(field(&listed, 0, 2), "reg+acme/json@1.2.0");
    assert_eq!(
        field(&listed, 0, 3),
        "sha256:2222222222222222222222222222222222222222222222222222222222222222"
    );
    assert_eq!(field(&listed, 1, 0), "acme/http");
    assert_eq!(field(&listed, 1, 2), "reg+acme/http@1.1.0");

    // Nothing has been fetched, so the store cannot satisfy the lockfile yet --
    // and `verify` says which entries are missing without going to look.
    let out = project.run(&app, &["verify"]);
    assert_eq!(out.status.code(), Some(NEEDS_INSTALLING));
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert_eq!(field(&stdout, 0, 0), "missing", "stdout was:\n{stdout}");

    // A local registry is already the index, so `update` fetches nothing.
    let updated = project.expect(&app, &["update"], READY);
    assert_eq!(field(&updated, 0, 0), "local");
    assert_eq!(field(&updated, 0, 1), "Testing");
}

/// A path dependency overrides the registry for the name it supplies.
#[test]
fn a_path_dependency_is_preferred_to_a_published_one() {
    let project = Project::new("override");
    project.publish(
        "acme/http",
        "1.1.0",
        "https://example.invalid/http.git",
        "4444444444444444444444444444444444444444",
        "4444444444444444444444444444444444444444444444444444444444444444",
        "",
    );
    // The same package, in a directory, at a version that satisfies the same
    // requirement. Without the override rule the solver would be offered both
    // and could take the published one, which is not what editing a checkout
    // beside your project means.
    std::fs::create_dir_all(project.dir("http").join("src")).expect("a package directory");
    std::fs::write(
        project.dir("http").join("ingot.toml"),
        "[package]\nname = \"acme/http\"\nversion = \"1.5.0\"\nroot = \"src/http.ws\"\n",
    )
    .expect("a manifest");
    std::fs::write(
        project.dir("http").join("src/http.ws"),
        "pub fn one() i64 { return 1; }\n",
    )
    .expect("a source file");

    let app = project.dir("app");
    std::fs::create_dir_all(&app).expect("an app directory");
    project.expect(&app, &["init", "myapp"], READY);
    project.expect(&app, &["add", "acme/http", "--path", "../http"], READY);
    project.expect(&app, &["resolve"], READY);

    let listed = project.expect(&app, &["list"], READY);
    assert_eq!(field(&listed, 0, 0), "acme/http");
    assert_eq!(
        field(&listed, 0, 1),
        "1.5.0",
        "the directory, not the registry"
    );
    assert_eq!(field(&listed, 0, 2), "path+../http");
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

/// The thing all of this was for: a program that imports a package.
///
/// One test rather than four, because what is interesting is the whole chain --
/// `resolve` chooses, `install` copies the tree in and writes down where it
/// landed, the loader reads that, and the type checker sees a module like any
/// other. The package has two files and presents one, which is what re-export
/// exists for: nothing outside can name `src/inside.ws` at all.
#[test]
fn a_program_imports_a_package_through_its_facade() {
    let project = Project::new("import");
    let util = project.package(
        "util",
        "0.3.0",
        "",
        "const inside = @import(\"./inside.ws\");\n\
         pub const Pair = inside.Pair;\n\
         pub const twice = inside.twice;\n",
    );
    project.beside(
        &util,
        "inside.ws",
        "pub const Pair = struct { a: i64, b: i64 };\n\
         pub fn twice(p: Pair) i64 { return (p.a + p.b) * 2; }\n",
    );
    let app = project.dir("app");
    std::fs::create_dir_all(app.join("src")).expect("an app directory");
    project.expect(&app, &["init", "myapp"], READY);
    project.expect(&app, &["add", "util", "--path", "../util"], READY);
    project.expect(&app, &["resolve"], READY);

    // Resolved is not installed, and the compiler says which verb is missing
    // rather than reporting a file it cannot find.
    std::fs::write(
        app.join("src").join("myapp.ws"),
        "const util = @import(\"util\");\n\
         fn main() i64 {\n\
         \x20   print_int(util.twice(util.Pair{ .a = 3, .b = 4 }));\n\
         \x20   return 0;\n\
         }\n",
    )
    .expect("a program");
    let out = project.compile(&app.join("src").join("myapp.ws"));
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        stderr.contains("run `ingot install`"),
        "an uninstalled project says which verb is missing:\n{stderr}"
    );

    project.expect(&app, &["install"], READY);
    assert!(app.join("ingot.env").exists(), "`install` writes it");

    // Through the tool, and through the compiler on its own from somewhere
    // else entirely. Both read the same file and neither needs the other.
    let out = project.run(&app, &["run", "src/myapp.ws"]);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "14\n");
    let out = project.compile(&app.join("src").join("myapp.ws"));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "14\n",
        "stderr was:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // What the facade does not rename cannot be reached, because nothing can
    // name the file it is in: a package is one module.
    std::fs::write(
        app.join("src").join("myapp.ws"),
        "const inside = @import(\"util/inside\");\nfn main() i64 { return 0; }\n",
    )
    .expect("a program");
    let out = project.compile(&app.join("src").join("myapp.ws"));
    assert_ne!(out.status.code(), Some(0), "a package presents one file");
}

/// A lockfile is the whole graph's, because the solver chooses one version of a
/// package for the whole project. What a package may *name* is narrower: what
/// its own manifest asked for, and nothing else.
#[test]
fn a_package_may_import_only_what_it_asked_for() {
    let project = Project::new("scope");
    project.package("core", "0.1.0", "", "pub fn one() i64 { return 1; }\n");
    project.package(
        "util",
        "0.3.0",
        "core = { path = \"../core\" }\n",
        "const core = @import(\"core\");\n\
         pub fn twice(n: i64) i64 { return n * 2 * core.one(); }\n",
    );
    let app = project.dir("app");
    std::fs::create_dir_all(app.join("src")).expect("an app directory");
    project.expect(&app, &["init", "myapp"], READY);
    project.expect(&app, &["add", "util", "--path", "../util"], READY);
    project.expect(&app, &["resolve"], READY);
    project.expect(&app, &["install"], READY);

    // `util` asked for `core`, so it may name it -- and does, in the answer.
    std::fs::write(
        app.join("src").join("myapp.ws"),
        "const util = @import(\"util\");\n\
         fn main() i64 { print_int(util.twice(21)); return 0; }\n",
    )
    .expect("a program");
    let out = project.compile(&app.join("src").join("myapp.ws"));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "42\n");

    // `myapp` did not, even though `core` is in its lockfile.
    std::fs::write(
        app.join("src").join("myapp.ws"),
        "const core = @import(\"core\");\nfn main() i64 { return core.one(); }\n",
    )
    .expect("a program");
    let out = project.compile(&app.join("src").join("myapp.ws"));
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        stderr.contains("`core` is not a dependency of `myapp`"),
        "a dependency of a dependency is not one of ours:\n{stderr}"
    );
}

/// An environment is a lockfile, and a lockfile is named by where it is.
///
/// `register` hashes the path it is handed, so handing it the bare `ingot.lock`
/// made every project on the machine the same environment: the second
/// `install` unregistered the first, and the next `gc` collected its entries.
/// Two projects in one store is the smallest thing that shows it.
#[test]
fn two_projects_in_one_store_keep_each_others_entries() {
    let project = Project::new("environments");
    project.package(
        "util",
        "0.3.0",
        "",
        "pub fn twice(n: i64) i64 { return n * 2; }\n",
    );
    project.package("other", "0.1.0", "", "pub fn nothing() i64 { return 0; }\n");

    let first = project.dir("first");
    let second = project.dir("second");
    for (dir, dep) in [(&first, "util"), (&second, "other")] {
        std::fs::create_dir_all(dir).expect("a project directory");
        project.expect(dir, &["init", "app"], READY);
        project.expect(dir, &["add", dep, "--path", &format!("../{dep}")], READY);
        project.expect(dir, &["resolve"], READY);
        project.expect(dir, &["install"], READY);
    }

    let store = project.expect(&first, &["store"], READY);
    assert_eq!(field(&store, 2, 1), "2", "one tree from each project");

    // Collecting from either project keeps both, because both are registered.
    let collected = project.expect(&first, &["gc"], READY);
    assert_eq!(field(&collected, 0, 1), "0", "nothing is unreachable");
    assert_eq!(field(&collected, 1, 1), "2");
    project.expect(&second, &["verify"], READY);
}

/// Resolving again invalidates the environment, and silently getting away with
/// it is the failure that would be hardest to see.
///
/// A new resolution names new store entries; the old ones are still there and
/// still hold what they always did, so a compiler reading the environment
/// `install` wrote last time would build the previous version of a dependency
/// and say nothing at all. `resolve` removes it, and the next compile asks for
/// the verb that has not been run.
#[test]
fn resolving_again_invalidates_the_environment() {
    let project = Project::new("stale-env");
    let util = project.package(
        "util",
        "0.3.0",
        "",
        "pub fn twice(n: i64) i64 { return n * 2; }\n",
    );
    let app = project.dir("app");
    std::fs::create_dir_all(app.join("src")).expect("an app directory");
    project.expect(&app, &["init", "myapp"], READY);
    project.expect(&app, &["add", "util", "--path", "../util"], READY);
    project.expect(&app, &["resolve"], READY);
    project.expect(&app, &["install"], READY);

    let program = app.join("src").join("myapp.ws");
    std::fs::write(
        &program,
        "const util = @import(\"util\");\n\
         fn main() i64 { print_int(util.twice(21)); return 0; }\n",
    )
    .expect("a program");
    assert_eq!(
        String::from_utf8_lossy(&project.compile(&program).stdout),
        "42\n"
    );

    std::fs::write(
        util.join("src").join("util.ws"),
        "pub fn twice(n: i64) i64 { return n * 3; }\n",
    )
    .expect("an edit");
    project.expect(&app, &["resolve"], READY);
    assert!(
        !app.join("ingot.env").exists(),
        "`resolve` takes the environment away rather than leaving one that lies"
    );

    let out = project.compile(&program);
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        stderr.contains("run `ingot install`"),
        "and the compiler asks for the verb that has not been run:\n{stderr}"
    );

    project.expect(&app, &["install"], READY);
    assert_eq!(
        String::from_utf8_lossy(&project.compile(&program).stdout),
        "63\n",
        "which then builds the version that was chosen"
    );
}
