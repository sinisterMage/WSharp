//! Process APIs through both compiler backends. Helpers are native W# programs,
//! so these checks require no shell and run on Windows as well as Unix.
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "wsharp-process-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&dir).unwrap();
        Self(dir)
    }
    fn source(&self, name: &str, source: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, source).unwrap();
        path
    }
    fn build(&self, source: &Path, name: &str) -> PathBuf {
        let path = self
            .0
            .join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        let out = Command::new(env!("CARGO_BIN_EXE_wsharp"))
            .arg("build")
            .arg(source)
            .arg("-o")
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        path
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const HELPER: &str = r#"
const os = @import("std/os");
const time = @import("std/time");
const text = @import("std/str");
const io = @import("std/io");
fn main() i64 {
    const a = os.args();
    if (text.eq(a[0], "args")) { print(a[1]); print(a[2]); print(os.get("PROCESS_TEST_VALUE") orelse "unset"); print(os.cwd() catch "bad cwd"); return 7; }
    if (text.eq(a[0], "streams")) {
        var i = 0;
        while (i < 40) : (i += 1) { print(text.repeat("x", 4095)); io.progress(text.repeat("y", 4095), true); }
        return 0;
    }
    if (text.eq(a[0], "sleep")) { time.sleep_ms(30000) catch return 3; return 0; }
    if (text.eq(a[0], "signal")) {
        os.catch_signals() catch return 4;
        io.write_file(a[1], "ready") catch return 5;
        while (true) {
            if (os.take_signal()) |s| { print(text.from_int(s)); os.restore_signals(); return 0; }
            time.sleep_ms(2) catch return 6;
        }
    }
    return 2;
}
"#;
const DRIVER: &str = r#"
const p = @import("std/process");
const os = @import("std/os");
const text = @import("std/str");
const time = @import("std/time");
fn main() i64 {
    const a = os.args();
    var c = p.command(a[0], []str{ "args", "a b", "$(must-not-run); `literal`" });
    c.cwd = a[1]; c.clear_env = true;
    c.env = []p.Env{ p.Env{ .name = "PROCESS_TEST_VALUE", .value = "value with spaces" } };
    const result = p.run(c, 5000) catch return 1;
    if (result.status.code != 7 or result.status.signal != 0 or p.success(result.status)) { return 2; }
    const want = text.concat("a b\n$(must-not-run); `literal`\nvalue with spaces\n", text.concat(a[1], "\n"));
    if (!text.eq(result.stdout, want) or !text.eq(result.stderr, "")) { print(result.stdout); return 3; }
    var streams = p.command(a[0], []str{ "streams" });
    const full = p.run(streams, 5000) catch return 4;
    if (!p.success(full.status) or text.len(full.stdout) != 163840 or text.len(full.stderr) != 163840) { return 5; }
    streams.output_limit = 1000;
    const limited = p.run(streams, 5000) catch |e| {
        if (e != error.OutputLimit) { return 6; }
        return lifecycle(a[0]);
    };
    return 7;
}
fn lifecycle(helper: str) i64 {
    const c = p.spawn(p.command(helper, []str{ "sleep" })) catch return 8;
    if (p.poll(c) catch return 9) |status| { p.close(c); return 10; }
    const start = time.monotonic_ms();
    const s = p.wait(c, 20) catch |e| {
        if (e != error.TimedOut) { p.close(c); return 11; }
        if (time.monotonic_ms() - start < 15) { p.close(c); return 12; }
        p.kill(c) catch { p.close(c); return 13; };
        const dead = p.collect(c, 5000) catch { p.close(c); return 14; };
        if (p.success(dead.status)) { p.close(c); return 15; }
        p.close(c); p.close(c);
        const stale = p.poll(c) catch |err| {
            if (err != error.IoFailed) { return 16; }
            print("captured, bounded, timed out, killed, reaped"); return 0;
        };
        return 17;
    };
    p.close(c); return 18;
}
"#;

fn bounded_output(mut cmd: Command) -> std::process::Output {
    // Kill and report on a regression instead of hanging the entire suite.
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if start.elapsed() > Duration::from_secs(30) {
            let _ = child.kill();
            let out = child.wait_with_output().unwrap();
            panic!("test timed out: {}", String::from_utf8_lossy(&out.stderr));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn capture_and_supervision_in_jit_aot_and_gc_stress() {
    let scratch = Scratch::new();
    let helper = scratch.build(&scratch.source("helper.ws", HELPER), "helper");
    let source = scratch.source("driver.ws", DRIVER);
    let driver = scratch.build(&source, "driver");
    for aot in [false, true] {
        for stress in [false, true] {
            let mut cmd = if aot {
                Command::new(&driver)
            } else {
                let mut c = Command::new(env!("CARGO_BIN_EXE_wsharp"));
                c.arg("run").arg(&source);
                c
            };
            if stress {
                cmd.env("WSHARP_GC_STRESS", "1");
            }
            cmd.arg(&helper).arg(&scratch.0);
            let out = bounded_output(cmd);
            assert!(
                out.status.success(),
                "aot={aot} stress={stress} {:?}\n{}\n{}",
                out.status,
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            assert_eq!(
                String::from_utf8_lossy(&out.stdout),
                "captured, bounded, timed out, killed, reaped\n"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn signals_reach_wsharp_without_an_async_wsharp_callback() {
    let scratch = Scratch::new();
    let helper = scratch.build(&scratch.source("helper.ws", HELPER), "helper");
    let source = scratch.source(
        "signal_driver.ws",
        r#"
const p = @import("std/process");
const os = @import("std/os");
const io = @import("std/io");
const time = @import("std/time");
const text = @import("std/str");
fn main() i64 {
    const a = os.args();
    const c = p.spawn(p.command(a[0], []str{ "signal", a[1] })) catch return 1;
    const until = time.monotonic_ms() + 5000;
    while (!io.exists(a[1])) {
        if (time.monotonic_ms() > until) { p.close(c); return 2; }
        time.sleep_ms(2) catch { p.close(c); return 3; };
    }
    p.terminate(c) catch { p.close(c); return 4; };
    const out = p.collect(c, 5000) catch { p.close(c); return 5; };
    p.close(c);
    if (!p.success(out.status) or !text.eq(out.stdout, "15\n")) { return 6; }
    return 0;
}
"#,
    );
    let driver = scratch.build(&source, "signal_driver");
    let mut cmd = Command::new(driver);
    cmd.arg(helper).arg(scratch.0.join("ready"));
    let out = bounded_output(cmd);
    assert!(
        out.status.success(),
        "{:?}: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_wsharp_program_can_capture_the_compilers_api_emit() {
    let scratch = Scratch::new();
    let api = scratch.source(
        "api.ws",
        "pub const Example = struct { id: i64 };\nfn main() i64 { return 0; }\n",
    );
    let driver = scratch.source("capture.ws", r#"
const p = @import("std/process");
const os = @import("std/os");
const text = @import("std/str");
fn main() i64 {
    const a = os.args();
    const out = p.run(p.command(a[0], []str{ "check", a[1], "--emit=api" }), 5000) catch return 1;
    if (!p.success(out.status) or !text.starts_with(out.stdout, "(api 1)\n") or !text.eq(out.stderr, "")) { return 2; }
    return 0;
}
"#);
    let driver = scratch.build(&driver, "capture");
    let mut cmd = Command::new(driver);
    cmd.arg(env!("CARGO_BIN_EXE_wsharp")).arg(api);
    let out = bounded_output(cmd);
    assert!(
        out.status.success(),
        "{:?}: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn normal_return_explicit_exit_and_panic_close_owned_children() {
    let scratch = Scratch::new();
    let helper = scratch.build(
        &scratch.source(
            "cleanup_helper.ws",
            r#"
const os = @import("std/os");
const io = @import("std/io");
const time = @import("std/time");
fn main() i64 {
    const a = os.args();
    io.write_file(a[0], "ready") catch return 1;
    time.sleep_ms(250) catch return 2;
    io.write_file(a[1], "escaped cleanup") catch return 3;
    return 0;
}
"#,
        ),
        "cleanup_helper",
    );
    for (name, leave, code) in [
        ("return", "return 0;", 0),
        ("exit", "os.exit(0); return 0;", 0),
        ("panic", "const xs = []i64{}; return xs[0];", 101),
    ] {
        let source = scratch.source(
            &format!("cleanup_{name}.ws"),
            &format!(
                r#"
const p = @import("std/process");
const os = @import("std/os");
const io = @import("std/io");
const time = @import("std/time");
fn main() i64 {{
    const a = os.args();
    const child = p.spawn(p.command(a[0], []str{{ a[1], a[2] }})) catch return 1;
    const until = time.monotonic_ms() + 5000;
    while (!io.exists(a[1])) {{
        if (time.monotonic_ms() > until) {{ p.close(child); return 2; }}
        time.sleep_ms(2) catch {{ p.close(child); return 3; }};
    }}
    {leave}
}}
"#
            ),
        );
        let driver = scratch.build(&source, &format!("cleanup_{name}"));
        for aot in [false, true] {
            let mut cmd = if aot {
                Command::new(&driver)
            } else {
                let mut c = Command::new(env!("CARGO_BIN_EXE_wsharp"));
                c.arg("run").arg(&source);
                c
            };
            let ready = scratch.0.join(format!("{name}-{aot}.ready"));
            let escaped = scratch.0.join(format!("{name}-{aot}.escaped"));
            cmd.arg(&helper).arg(&ready).arg(&escaped);
            let out = bounded_output(cmd);
            assert_eq!(
                out.status.code(),
                Some(code),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(ready.exists());
            std::thread::sleep(Duration::from_millis(300));
            assert!(!escaped.exists(), "child escaped {name}, aot={aot}");
        }
    }
}
