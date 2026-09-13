//! Real C libraries through both backends, also with a moving collector.
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn success(out: Output) -> Output {
    assert!(
        out.status.success(),
        "status: {}\nstdout:\n{}\nstderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

#[test]
fn native_signatures_buffers_gc_and_library_lifetime() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = std::env::temp_dir().join(format!("wsharp-ffi-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // A Unicode path exercises UTF-16 loading on Windows.
    let library = dir.join(if cfg!(windows) {
        "nativé 東京 😀.dll"
    } else if cfg!(target_os = "macos") {
        "libnativé 東京 😀.dylib"
    } else {
        "libnativé 東京 😀.so"
    });
    let mut cc = Command::new(std::env::var("CC").unwrap_or_else(|_| "cc".into()));
    cc.arg(if cfg!(target_os = "macos") {
        "-dynamiclib"
    } else {
        "-shared"
    });
    if !cfg!(windows) {
        cc.arg("-fPIC");
    }
    success(
        cc.arg(root.join("tests/fixtures/ffi.c"))
            .arg("-o")
            .arg(&library)
            .output()
            .unwrap(),
    );
    let source = root.join("tests/fixtures/ffi.ws");
    let wsharp = env!("CARGO_BIN_EXE_wsharp");
    success(
        Command::new(wsharp)
            .arg("check")
            .arg(&source)
            .output()
            .unwrap(),
    );
    let executable: PathBuf = dir.join(format!("ffi{}", std::env::consts::EXE_SUFFIX));
    success(
        Command::new(wsharp)
            .arg("build")
            .arg(&source)
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap(),
    );
    for aot in [false, true] {
        for stress in [false, true] {
            for closed in [false, true] {
                let mut command = if aot {
                    Command::new(&executable)
                } else {
                    let mut c = Command::new(wsharp);
                    c.arg("run").arg(&source).arg("--");
                    c
                };
                if stress {
                    command.env("WSHARP_GC_STRESS", "1");
                }
                let out = command
                    .arg(&library)
                    .arg(if closed { "closed" } else { "open" })
                    .output()
                    .unwrap();
                assert!(
                    out.status.code() == Some(if closed { 101 } else { 0 }),
                    "aot={aot} stress={stress} closed={closed}: {}\nstdout:\n{}\nstderr:\n{}",
                    out.status,
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr)
                );
                if closed {
                    assert_eq!(
                        out.status.code(),
                        Some(101),
                        "{}",
                        String::from_utf8_lossy(&out.stderr)
                    );
                    assert!(String::from_utf8_lossy(&out.stderr).contains("closed library"));
                } else {
                    let out = success(out);
                    assert_eq!(String::from_utf8_lossy(&out.stdout), "ffi ok\n");
                }
            }
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

/// The DLL path failure is a startup argv bug, so exercise argument decoding
/// separately from loading a library. Include a supplementary-plane character,
/// empty arguments, quotes, and trailing backslashes in both backends.
#[test]
fn command_line_arguments_preserve_unicode_and_quoting() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = std::env::temp_dir().join(format!("wsharp-argv-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let source = root.join("tests/fixtures/argv.ws");
    let executable = dir.join(format!("argv{}", std::env::consts::EXE_SUFFIX));
    let compiler = env!("CARGO_BIN_EXE_wsharp");
    success(
        Command::new(compiler)
            .arg("build")
            .arg(&source)
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap(),
    );
    let args = [
        "",
        "nativé.dll",
        "東京/😀",
        "spaces and \"quotes\"",
        "trailing\\",
        "back\\\"slash",
        "",
    ];
    let expected = format!("{}\n", args.join("\n"));
    for aot in [false, true] {
        for stress in [false, true] {
            let mut cmd = if aot {
                Command::new(&executable)
            } else {
                let mut c = Command::new(compiler);
                c.arg("run").arg(&source).arg("--");
                c
            };
            if stress {
                cmd.env("WSHARP_GC_STRESS", "1");
            }
            let out = success(cmd.args(args).output().unwrap());
            assert_eq!(out.stdout, expected.as_bytes(), "aot={aot} stress={stress}");
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}
