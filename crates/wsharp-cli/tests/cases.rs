//! End-to-end tests: compile and run every `.ws` file in `tests/cases`.
//!
//! Each case declares its own expectations in a header comment, so adding a
//! test means adding one file:
//!
//! ```text
//! // expect: 55          one line of expected stdout, in order
//! // exit: 3             expected exit status (default 0)
//! // error: <substring>  the program must fail to compile, saying this
//! ```
//!
//! Running the built binary as a subprocess means stdout is captured for free,
//! and the test exercises exactly what a user would run.

use std::path::{Path, PathBuf};
use std::process::Command;

struct Expectations {
    stdout: Vec<String>,
    exit: i32,
    error: Option<String>,
}

fn parse_expectations(source: &str) -> Expectations {
    let mut stdout = Vec::new();
    let mut exit = 0;
    let mut error = None;
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
            error = Some(v.trim().to_string());
        }
    }
    Expectations {
        stdout,
        exit,
        error,
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
        .output()
        .map_err(|e| format!("could not run the compiler: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if let Some(needle) = &expected.error {
        if output.status.success() {
            return Err(format!(
                "expected a compile error containing {needle:?}, but it ran"
            ));
        }
        if !stderr.contains(needle.as_str()) {
            return Err(format!(
                "expected an error containing {needle:?}, got:\n{stderr}"
            ));
        }
        return Ok(());
    }

    if !stderr.is_empty() {
        return Err(format!("unexpected stderr:\n{stderr}"));
    }

    let actual: Vec<&str> = stdout.lines().collect();
    if actual != expected.stdout {
        return Err(format!(
            "stdout mismatch\n  expected: {:?}\n  actual:   {:?}",
            expected.stdout, actual
        ));
    }

    let code = output.status.code().unwrap_or(-1);
    if code != expected.exit {
        return Err(format!(
            "expected exit status {}, got {code}",
            expected.exit
        ));
    }
    Ok(())
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
fn a_missing_file_is_reported_not_panicked() {
    let output = Command::new(env!("CARGO_BIN_EXE_wsharp"))
        .arg("run")
        .arg("does-not-exist.ws")
        .output()
        .expect("could not run the compiler");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot read"));
}
