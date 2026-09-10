//! `--emit=api`, pinned.
//!
//! The point of this emit is that a tool can be written against it, and the
//! point of this test is that the tool keeps working. `--emit=ast` carries no
//! such promise and has no such test: it is a debugging aid, it is shared with
//! the parser tests, and it prints whatever the syntax tree happens to hold.
//!
//! So the whole output for a small two-module program is written out below. A
//! change to it fails here, loudly, rather than quietly in somebody's
//! generator -- and a change that could make an existing reader wrong is a
//! change to `api::VERSION` as well.
//!
//! Note that the root module is called `"main"` rather than by its file path.
//! That is the loader's name for it and is what every other emit already says.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A directory of its own per test, with the process id in the name, removed at
/// the end. The emit carries absolute module paths, so two tests sharing a
/// directory would see each other's files.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wsharp-api-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a directory to write in");
    dir
}

/// The emit, with the scratch directory's path replaced by `DIR` -- otherwise
/// the expectation would be about where the test happened to run.
///
/// The needle is built the way the loader builds a module path rather than from
/// `dir` as it was spelled here, and both halves of that matter on Windows:
/// `canonicalize` resolves the short 8.3 form `temp_dir` may hand back, and
/// `module_path_of` writes the result with `/` separators. Calling the loader's
/// own function is the point -- a second copy of that rule here would be a
/// second thing to keep in step, and this test exists to notice when the first
/// one changes.
fn emit(dir: &Path, root: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_wsharp"))
        .arg("check")
        .arg(dir.join(root))
        .arg("--emit=api")
        .output()
        .expect("wsharp runs");
    assert!(
        out.status.success(),
        "`--emit=api` failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let canonical = dir.canonicalize().expect("the scratch directory is there");
    let needle = wsharp_cli::load::module_path_of(&canonical);
    String::from_utf8_lossy(&out.stdout).replace(&needle, "DIR")
}

#[test]
fn the_api_emit_is_what_it_says_it_is() {
    let dir = scratch("surface");
    std::fs::write(
        dir.join("fw.ws"),
        r#"
pub const Route = struct { };
pub const Response = struct { code: i64, body: str };
pub fn ok(b: str) Response { return Response{ .code = 200, .body = b }; }
"#,
    )
    .expect("a library module");
    std::fs::write(
        dir.join("app.ws"),
        r#"
const fw = @import("./fw.ws");
pub const ROUTE_Show = "GET /users/:id";
const LIMIT = 64;
pub const Show = struct : fw.Route { id: i64, page: ?i64 };
pub fn action(r: Show) fw.Response { return fw.ok("x"); }
pub fn first[T](xs: []T) ?T { if (0 < 1) { return xs[0]; } return null; }
pub fn risky(n: i64) !{BadFormat}i64 { if (n < 0) { return error.BadFormat; } return n; }
pub const alias_ok = fw.ok;
fn main() i64 { return 0; }
"#,
    )
    .expect("a root module");

    let expected = r#"(api 1)
(module "main"
  (import fw "DIR/fw.ws")
  (pub const ROUTE_Show (str "GET /users/:id"))
  (const LIMIT (int 64))
  (pub struct Show (parent "DIR/fw.ws".Route)
    (field id i64)
    (field page (optional i64)))
  (pub fn action
    (param r "main".Show)
    (ret "DIR/fw.ws".Response))
  (pub fn first (generics T)
    (param xs (array T))
    (ret (optional T)))
  (pub fn risky
    (param n i64)
    (ret (errunion i64 (errors BadFormat))))
  (pub const alias_ok (alias "DIR/fw.ws".ok))
  (fn main
    (ret i64)))
(module "DIR/fw.ws"
  (pub struct Route)
  (pub struct Response
    (field code i64)
    (field body str))
  (pub fn ok
    (param b str)
    (ret "DIR/fw.ws".Response)))
"#;
    assert_eq!(emit(&dir, "app.ws"), expected);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The one thing a reader has to be able to do that `--emit=ast` cannot help
/// with: follow a name to the module that declares it, through a facade that
/// declares nothing itself.
#[test]
fn a_reexport_names_the_module_it_came_from() {
    let dir = scratch("reexport");
    std::fs::write(
        dir.join("inner.ws"),
        "pub const Pair = struct { a: i64 };\npub fn twice(n: i64) i64 { return n * 2; }\n",
    )
    .expect("an inner module");
    std::fs::write(
        dir.join("facade.ws"),
        "const inner = @import(\"./inner.ws\");\n\
         pub const Pair = inner.Pair;\n\
         pub const twice = inner.twice;\n",
    )
    .expect("a facade");
    std::fs::write(
        dir.join("root.ws"),
        "const f = @import(\"./facade.ws\");\nfn main() i64 { return f.twice(1); }\n",
    )
    .expect("a root");

    let out = emit(&dir, "root.ws");
    assert!(
        out.contains("(pub const Pair (alias \"DIR/inner.ws\".Pair))"),
        "a re-exported type names its origin:\n{out}"
    );
    assert!(
        out.contains("(pub const twice (alias \"DIR/inner.ws\".twice))"),
        "a re-exported function names its origin:\n{out}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `--emit=obj` is `build`'s alone. It was accepted by every verb and handled
/// only by `build`, so `check --emit=obj` fell through every branch and *ran*
/// the program.
#[test]
fn emit_obj_is_refused_where_there_is_no_object() {
    let dir = scratch("obj");
    std::fs::write(
        dir.join("p.ws"),
        "fn main() i64 { print(\"this must not run\"); return 0; }\n",
    )
    .expect("a program");
    for verb in ["check", "run"] {
        let out = Command::new(env!("CARGO_BIN_EXE_wsharp"))
            .arg(verb)
            .arg(dir.join("p.ws"))
            .arg("--emit=obj")
            .output()
            .expect("wsharp runs");
        assert!(
            !out.status.success(),
            "`{verb} --emit=obj` should be refused"
        );
        assert!(
            !String::from_utf8_lossy(&out.stdout).contains("this must not run"),
            "`{verb} --emit=obj` ran the program"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
