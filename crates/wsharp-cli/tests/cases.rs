//! End-to-end tests: compile and run every `.ws` file in `tests/cases`.
//!
//! Each case declares its own expectations in a header comment, so adding a
//! test means adding one file:
//!
//! ```text
//! // expect: 55          one line of expected stdout, in order
//! // exit: 3             expected exit status (default 0)
//! // error: <substring>  the program must fail to compile, saying this
//! // panic: <substring>  the program must die with a W# panic saying this
//! // args: one two       what the program sees as `os.args()`
//! ```
//!
//! `args:` splits on whitespace, so an argument containing a space cannot be
//! written -- which is a limitation rather than a decision, and the day a case
//! needs one is the day to give the line a quoting rule.
//!
//! `error:` may be given more than once; every substring must then appear.
//! A `panic:` case must exit with status 101 (the runtime's panic status) and
//! is still held to its `expect:` lines, so it can check what was printed
//! before the panic; only for these cases is stderr allowed to be non-empty.
//!
//! Running the built binary as a subprocess means stdout is captured for free,
//! and the test exercises exactly what a user would run.

use std::path::{Path, PathBuf};
use std::process::Command;

/// What `ws_panic` exits with. Spelled out here rather than imported: the
/// harness should see exactly what a user sees.
const PANIC_EXIT_STATUS: i32 = 101;

struct Expectations {
    stdout: Vec<String>,
    exit: i32,
    errors: Vec<String>,
    panic: Option<String>,
    args: Vec<String>,
}

fn parse_expectations(source: &str) -> Expectations {
    let mut stdout = Vec::new();
    let mut exit = 0;
    let mut errors = Vec::new();
    let mut panic = None;
    let mut args = Vec::new();
    for line in source.lines() {
        let Some(rest) = line.trim_start().strip_prefix("//") else {
            continue;
        };
        let rest = rest.trim();
        if let Some(v) = rest.strip_prefix("expect:") {
            stdout.push(v.trim_start_matches(' ').to_string());
        } else if let Some(v) = rest.strip_prefix("exit:") {
            exit = v.trim().parse().expect("`exit:` needs a number");
        } else if let Some(v) = rest.strip_prefix("error:") {
            errors.push(v.trim().to_string());
        } else if let Some(v) = rest.strip_prefix("panic:") {
            panic = Some(v.trim().to_string());
        } else if let Some(v) = rest.strip_prefix("args:") {
            args.extend(v.split_whitespace().map(str::to_string));
        }
    }
    Expectations {
        stdout,
        exit,
        errors,
        panic,
        args,
    }
}

fn cases_dir() -> PathBuf {
    // <workspace>/crates/wsharp-cli -> <workspace>/tests/cases
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/cases")
}

fn check_case(path: &Path) -> Result<(), String> {
    check_case_with(path, &[])
}

fn check_case_with(path: &Path, flags: &[&str]) -> Result<(), String> {
    let source = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let expected = parse_expectations(&source);

    let output = Command::new(env!("CARGO_BIN_EXE_wsharp"))
        .arg("run")
        .args(flags)
        .arg(path)
        // After the file, because `run` collects its trailing arguments -- the
        // same way a user would pass them.
        .args(&expected.args)
        .output()
        .map_err(|e| format!("could not run the compiler: {e}"))?;

    verify(
        &expected,
        output.status.code().unwrap_or(-1),
        output.status.success(),
        &String::from_utf8_lossy(&output.stdout),
        &String::from_utf8_lossy(&output.stderr),
    )
}

/// Hold one run of a case to its header, whichever way it was run.
fn verify(
    expected: &Expectations,
    code: i32,
    success: bool,
    stdout: &str,
    stderr: &str,
) -> Result<(), String> {
    if !expected.errors.is_empty() {
        if success {
            return Err(format!(
                "expected a compile error containing {:?}, but it ran",
                expected.errors
            ));
        }
        for needle in &expected.errors {
            if !stderr.contains(needle.as_str()) {
                return Err(format!(
                    "expected an error containing {needle:?}, got:\n{stderr}"
                ));
            }
        }
        return Ok(());
    }

    match &expected.panic {
        Some(needle) => {
            if code != PANIC_EXIT_STATUS {
                return Err(format!(
                    "expected a panic (exit status {PANIC_EXIT_STATUS}), got {code}; stderr:\n{stderr}"
                ));
            }
            if !stderr.contains(needle.as_str()) {
                return Err(format!(
                    "expected a panic containing {needle:?}, got:\n{stderr}"
                ));
            }
        }
        None => {
            if !stderr.is_empty() {
                return Err(format!("unexpected stderr:\n{stderr}"));
            }
        }
    }

    let actual: Vec<&str> = stdout.lines().collect();
    if actual != expected.stdout {
        return Err(format!(
            "stdout mismatch\n  expected: {:?}\n  actual:   {:?}",
            expected.stdout, actual
        ));
    }

    if expected.panic.is_none() && code != expected.exit {
        return Err(format!(
            "expected exit status {}, got {code}",
            expected.exit
        ));
    }
    Ok(())
}

/// Build a case into a native executable and run *that*.
///
/// The other passes exercise the JIT. This exercises everything the JIT does
/// not: the object backend, the three tables written as data and read back at
/// startup, the relocations a linker fills in, and the `main` in
/// `wsharp-start`. A case that passes under `run` and fails here is a bug in
/// exactly that seam.
fn check_case_native(path: &Path, dir: &Path, env: &[(&str, &str)]) -> Result<(), String> {
    let source = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let expected = parse_expectations(&source);
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let exe = dir.join(&*stem);

    let built = Command::new(env!("CARGO_BIN_EXE_wsharp"))
        .arg("build")
        .arg(path)
        .arg("-o")
        .arg(&exe)
        .output()
        .map_err(|e| format!("could not run the compiler: {e}"))?;

    // A case that must not compile is answered here, and `build` has to say
    // the same thing `run` would -- which it does by sharing the front end.
    if !expected.errors.is_empty() || !built.status.success() {
        return verify(
            &expected,
            built.status.code().unwrap_or(-1),
            built.status.success(),
            &String::from_utf8_lossy(&built.stdout),
            &String::from_utf8_lossy(&built.stderr),
        );
    }

    let mut command = Command::new(&exe);
    command.args(&expected.args);
    for (k, v) in env {
        command.env(k, v);
    }
    let output = command
        .output()
        .map_err(|e| format!("could not run {}: {e}", exe.display()))?;
    // Removed on success only, so a failure leaves something to run by hand.
    let result = verify(
        &expected,
        output.status.code().unwrap_or(-1),
        output.status.success(),
        &String::from_utf8_lossy(&output.stdout),
        &String::from_utf8_lossy(&output.stderr),
    );
    if result.is_ok() {
        let _ = std::fs::remove_file(&exe);
    }
    result
}

/// Somewhere to put the executables this builds.
///
/// Carries the process id because the suite's passes may run at the same time
/// as each other, and two of them building `fib` into one path would race.
fn native_dir(what: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wsharp-aot-{what}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("cannot make {}: {e}", dir.display()));
    dir
}

fn case_files() -> Vec<PathBuf> {
    let dir = cases_dir();
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "ws"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no cases found in {}", dir.display());
    paths
}

#[test]
fn every_case_behaves_as_declared() {
    let paths = case_files();

    // Run them all before reporting, so one break does not hide the others.
    let mut failures = Vec::new();
    for path in &paths {
        if let Err(message) = check_case(path) {
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            failures.push(format!("--- {name} ---\n{message}"));
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} cases failed:\n\n{}",
        failures.len(),
        paths.len(),
        failures.join("\n\n")
    );
}

#[test]
fn examples_compile_and_run() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "ws"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no examples found");

    let mut failures = Vec::new();
    for path in &paths {
        let output = Command::new(env!("CARGO_BIN_EXE_wsharp"))
            .arg("run")
            .arg(path)
            .output()
            .expect("could not run the compiler");
        if !output.status.success() {
            failures.push(format!(
                "--- {} ---\n{}",
                path.display(),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "examples failed:\n\n{}",
        failures.join("\n\n")
    );
}

/// The whole suite again, collecting at every allocation.
///
/// This is the main defence for the collector's roots. Rooting is spread over
/// every expression the code generator lowers, and the failure mode of getting
/// it wrong -- a live object freed or moved without its references being
/// updated -- is rare and unreproducible in normal running. Under stress it
/// becomes an immediate abort on the very first allocation that follows the
/// mistake, in whichever case exercises it.
#[test]
fn every_case_survives_collecting_at_every_allocation() {
    let mut failures = Vec::new();
    for path in case_files() {
        if let Err(message) = check_case_with(&path, &["--gc-stress"]) {
            failures.push(format!("--- {} ---\n{message}", path.display()));
        }
    }
    assert!(
        failures.is_empty(),
        "cases failed under --gc-stress:\n\n{}",
        failures.join("\n\n")
    );
}

/// The whole suite again, compiled to native executables.
///
/// Everything above runs the program inside the compiler, where the runtime is
/// already there to be handed the type registry, the stack maps and the
/// service table. A built program has none of that: the three tables travel as
/// data, their code addresses are relocations, and a `main` in `wsharp-start`
/// installs them before anything runs. None of that is exercised by a JIT pass,
/// and a backend nothing exercises is one that does not work.
#[test]
fn every_case_behaves_the_same_built_as_run() {
    let dir = native_dir("cases");
    let mut failures = Vec::new();
    for path in case_files() {
        if let Err(message) = check_case_native(&path, &dir, &[]) {
            failures.push(format!("--- {} ---\n{message}", path.display()));
        }
    }
    assert!(
        failures.is_empty(),
        "cases failed when built:\n\n{}",
        failures.join("\n\n")
    );
}

/// The collector's cases, built, and collecting at every allocation.
///
/// The stack maps are the table where a serialisation mistake is silent: the
/// collector would read a root at the wrong stack offset and mark whatever
/// happened to be there. Stress turns that from a rare corruption into an
/// abort on the next allocation. Only the `gc_*` cases, because they are the
/// ones that allocate hard enough to say anything and the whole suite twice
/// over is a cost without a matching return.
#[test]
fn the_collector_survives_stress_in_a_built_program() {
    let dir = native_dir("stress");
    let cases: Vec<PathBuf> = case_files()
        .into_iter()
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("gc_"))
        })
        .collect();
    assert!(!cases.is_empty(), "no `gc_*` cases found");

    let mut failures = Vec::new();
    for path in &cases {
        if let Err(message) = check_case_native(path, &dir, &[("WSHARP_GC_STRESS", "1")]) {
            failures.push(format!("--- {} ---\n{message}", path.display()));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} built `gc_*` cases failed under WSHARP_GC_STRESS:\n\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n\n")
    );
}

/// A built program's collector must actually do something.
///
/// Checking that a case passes is not enough: a stack walk that found no roots
/// at all would make every root check succeed for the wrong reason, and a
/// stack-map table that deserialised to nothing looks exactly like that. So
/// read the counts and insist they are not zero -- the same discipline the
/// collector's own notes ask for.
#[test]
fn a_built_program_reports_the_collector_doing_its_work() {
    let dir = native_dir("stats");
    let exe = dir.join("gc_moving");
    let built = Command::new(env!("CARGO_BIN_EXE_wsharp"))
        .arg("build")
        .arg(cases_dir().join("gc_moving.ws"))
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("could not run the compiler");
    assert!(
        built.status.success(),
        "could not build gc_moving.ws:\n{}",
        String::from_utf8_lossy(&built.stderr)
    );

    let output = Command::new(&exe)
        .env("WSHARP_GC_STATS", "1")
        .output()
        .expect("could not run the built program");
    let stats = String::from_utf8_lossy(&output.stderr);
    let line = stats
        .lines()
        .find(|l| l.starts_with("W# gc:"))
        .unwrap_or_else(|| panic!("no collector report; stderr was:\n{stats}"));

    // A number followed by the words the report uses, so this reads the same
    // way the line does.
    let number_before = |what: &str| -> usize {
        let at = line
            .find(what)
            .unwrap_or_else(|| panic!("no `{what}` in the report:\n{line}"));
        line[..at]
            .split_whitespace()
            .next_back()
            .and_then(|n| n.parse().ok())
            .unwrap_or_else(|| panic!("no number before `{what}` in:\n{line}"))
    };

    assert!(
        number_before("traces") > 0,
        "no trace ever started:\n{line}"
    );
    assert!(
        number_before("moved") > 0,
        "nothing was ever moved:\n{line}"
    );
    assert!(
        number_before("roots seen") > 0,
        "the stack walk found no roots at all, which would make every root \
         check pass vacuously -- the stack maps most likely did not survive \
         being written to the object file:\n{line}"
    );
    assert!(
        number_before("safepoints") > 0,
        "no safepoints are registered:\n{line}"
    );
    let _ = std::fs::remove_file(&exe);
}

#[test]
fn check_reports_errors_without_running() {
    let path = cases_dir().join("err_type_mismatch.ws");
    let output = Command::new(env!("CARGO_BIN_EXE_wsharp"))
        .arg("check")
        .arg(&path)
        .output()
        .expect("could not run the compiler");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("type mismatch"));
}

#[test]
fn emit_types_prints_inferred_signatures() {
    let path = cases_dir().join("generics.ws");
    let output = Command::new(env!("CARGO_BIN_EXE_wsharp"))
        .arg("check")
        .arg(&path)
        .arg("--emit=types")
        .output()
        .expect("could not run the compiler");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("id: fn(T) T"), "{stdout}");
    assert!(stdout.contains("main: fn() i64"), "{stdout}");
}

#[test]
fn emit_clif_prints_cranelift_ir() {
    let path = cases_dir().join("recursion.ws");
    let output = Command::new(env!("CARGO_BIN_EXE_wsharp"))
        .arg("run")
        .arg(&path)
        .arg("--emit=clif")
        .output()
        .expect("could not run the compiler");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("function u0:"), "{stdout}");
    assert!(stdout.contains("; fib"), "{stdout}");
}

#[test]
fn emit_tokens_still_reports_lexer_errors() {
    // A scratch file: the token dump is the one path that re-lexes on its
    // own, and it used to drop the lexer's diagnostics on the floor.
    let path = std::env::temp_dir().join(format!("wsharp-emit-tokens-{}.ws", std::process::id()));
    std::fs::write(&path, "fn main() i64 { print(\"open; return 0; }\n")
        .expect("write scratch file");
    let output = Command::new(env!("CARGO_BIN_EXE_wsharp"))
        .arg("check")
        .arg(&path)
        .arg("--emit=tokens")
        .output()
        .expect("could not run the compiler");
    let _ = std::fs::remove_file(&path);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    // The tokens are still printed; the error follows rather than replaces them.
    assert!(stdout.contains("Fn"), "{stdout}");
    assert!(stderr.contains("unterminated"), "{stderr}");
}

#[test]
fn help_documents_the_collector_environment_variables() {
    let output = Command::new(env!("CARGO_BIN_EXE_wsharp"))
        .arg("--help")
        .output()
        .expect("could not run the compiler");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("WSHARP_GC_STATS"), "{stdout}");
    assert!(stdout.contains("WSHARP_GC_TRACE"), "{stdout}");
}

#[test]
fn a_missing_file_is_reported_not_panicked() {
    let output = Command::new(env!("CARGO_BIN_EXE_wsharp"))
        .arg("run")
        .arg("does-not-exist.ws")
        .output()
        .expect("could not run the compiler");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot read"));
}
