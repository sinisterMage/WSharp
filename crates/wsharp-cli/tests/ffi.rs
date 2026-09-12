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
        "nativé.dll"
    } else if cfg!(target_os = "macos") {
        "libnativé.dylib"
    } else {
        "libnativé.so"
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
