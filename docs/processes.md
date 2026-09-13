# Run and supervise child processes

Use `std/process` to run external commands, capture their output, or manage a
server alongside your application. These APIs are available in W# 0.1.8 and
work with both `wsharp run` and compiled executables.

## Run a command

`process.run` starts a command, waits for its output, and releases the child
process when it finishes or an error occurs.

```wsharp
const process = @import("std/process");

fn main() i64 {
    const cmd = process.command("wsharp", []str{ "--version" });
    const output = process.run(cmd, 5000) catch return 1;
    if (!process.success(output.status)) {
        print(output.stderr);
        return 1;
    }
    print(output.stdout);
    return 0;
}
```

Pass each argument as a separate array element. Spaces, quotes, `*`, and `$`
are passed to the program without shell expansion. On Windows, prefer calling
an executable directly; batch files and command interpreters have their own
argument-parsing rules.

A nonzero exit code is a normal result. Check `process.success(output.status)`
to decide whether the command succeeded. `Status.code` contains its exit code;
when a Unix signal terminates the child, `code` is -1 and `Status.signal`
contains the signal number. Otherwise `signal` is zero.

## Configure a command

`process.command(program, args)` returns a mutable `Command`. Configure it
before passing it to `run` or `spawn`:

```wsharp
const process = @import("std/process");

fn main() i64 {
    var cmd = process.command("wsharp", []str{ "check", "app/main.ws", "--emit=api" });
    cmd.cwd = "/path/to/project";
    cmd.env = []process.Env{
        process.Env{ .name = "APP_ENV", .value = "test" },
    };
    cmd.output_limit = 16777216;
    const output = process.run(cmd, 30000) catch return 1;
    if (!process.success(output.status)) { print(output.stderr); return 1; }
    print(output.stdout);
    return 0;
}
```

| Field | Default | Purpose |
|---|---|---|
| `program`, `args` | Supplied to `command` | Executable and its arguments |
| `cwd` | `""` | Child's working directory; empty inherits yours |
| `env` | `[]process.Env{}` | Environment variables to add or override |
| `clear_env` | `false` | Start with an empty environment before applying `env` |
| `stdin` | `process.Null` | Read EOF immediately, or use `process.Inherit` |
| `stdout`, `stderr` | `process.Capture` | Capture, inherit, or discard each stream |
| `output_limit` | `8388608` | Maximum combined captured bytes: 8 MiB by default |
| `group` | `false` | Start a separate Unix process group |

Directory and environment settings affect only the child. `process.Null`
discards output; `process.Inherit` connects it to your program's corresponding
stream. Piped stdin is not currently supported.

## Manage a running process

Use `spawn` when you need to do other work while a child runs. Close every child
you spawn, including on error paths. For a long-running service, inherit its
output so logs appear directly without accumulating in a capture buffer.

```wsharp
const process = @import("std/process");
const os = @import("std/os");
const time = @import("std/time");

fn main() i64 {
    os.catch_signals() catch return 1;
    var cmd = process.command("./app-server", []str{});
    cmd.stdout = process.Inherit;
    cmd.stderr = process.Inherit;
    const child = process.spawn(cmd) catch return 2;

    while (true) {
        if (process.poll(child) catch { process.close(child); return 3; }) |status| {
            process.close(child);
            return status.code;
        }
        if (os.take_signal()) |signal| { break; }
        time.sleep_ms(20) catch { process.close(child); return 4; };
    }

    // Ask the server to stop, allowing five seconds for graceful shutdown.
    process.terminate(child) catch { process.close(child); return 5; };
    const status = process.wait(child, 5000) catch {
        process.close(child);
        return 6;
    };
    process.close(child);
    return status.code;
}
```

This example uses Unix SIGTERM for graceful shutdown. The server must handle
that signal itself. On Windows, use your application's cooperative shutdown
channel, then `kill` or `close` if the child does not stop.

| Function | Result |
|---|---|
| `spawn(command)` | A `Child` handle |
| `poll(child)` | `?Status`: null while the child is running |
| `wait(child, timeout_ms)` | `Status` when the child exits |
| `collect(child, timeout_ms)` | `Output` after exit and the end of both captured streams |
| `run(command, timeout_ms)` | Spawn, collect, and close in one call |
| `terminate(child)` | Send Unix SIGTERM |
| `interrupt(child)` | Send Unix SIGINT |
| `kill(child)` | Request forced termination |
| `close(child)` | Kill if still running, wait for exit, and release resources |

`close` is safe to call more than once. Other operations on a closed handle
return `IoFailed`. Normal return from your program's `main` also closes any
remaining children. Explicit cleanup lets you control when they stop;
`os.exit` and runtime panic only attempt emergency termination.

## Timeouts and output limits

Timeouts are in milliseconds. A negative value waits indefinitely; zero checks
once without waiting. They use a monotonic clock, so changing the system clock
does not change the allotted duration.

`wait` and `collect` return `TimedOut` without closing the child. You can wait
again, request termination, or close it. `run` closes its child on every path.
Timeouts start after spawning and exclude cleanup; an OS operation that cannot
be interrupted can delay the wait performed by `close`.

`wait` finishes when the direct child exits. `collect` also waits for captured
stdout and stderr to end. If a descendant keeps either stream open, `collect`
can time out even after `wait` has returned a status.

Captured stdout and stderr share `output_limit`, which accepts 0 through 1 GiB.
Exceeding it requests termination and returns `OutputLimit`; truncated output
is never reported as a successful capture. Captured data is kept until close,
and polling does not reset the limit. Both streams are read during `poll`,
`wait`, and `collect`. Between those calls, a child can block when its output
pipe fills. Use inherited or discarded output for unattended services.

## Handle shutdown signals

Call `os.catch_signals()` to receive shutdown requests through
`os.take_signal()`. It returns `?i64`: `os.Interrupt`, `os.Terminate`, or null
when nothing is pending. Repeated pending signals of the same kind are combined.
Use one consumer in your main loop and notify your workers from there.

On Unix, this handles SIGINT and SIGTERM. On Windows, console Ctrl-C and
Ctrl-Break produce `os.Interrupt`. Windows service stops, window closure,
logoff, and system shutdown require their own handling.

`os.restore_signals()` restores the previous handlers and clears pending
signals. Normal main return restores them too. Signal handling is process-wide;
coordinate it with any native libraries that handle the same signals.

Use `time.monotonic_ms()` to measure durations and `time.sleep_ms(n)` to pause
without blocking garbage collection. Negative sleep durations are rejected.
Sleep is not interrupted by these signal notifications, so keep polling
intervals short. `time.now()` continues to return seconds since the Unix epoch.

## Platform support and errors

Spawning, capture, polling, waits, and forced termination are supported on
Windows and Unix. `terminate`, `interrupt`, and `group = true` require Unix;
Windows reports `NotSupported` for unsupported operations.

With `group = true`, signals and close target the child's process group until
its leader has been reaped by poll/wait/collect. Afterward, signals are no-ops.
Descendants that outlive their leader or leave the group need separate
supervision. Windows process-tree management is not provided.

| Error | Meaning |
|---|---|
| `NotFound` | The executable or requested working directory was not found |
| `PermissionDenied` | The OS refused permission to start the command |
| `BadFormat` | Invalid command options, an embedded NUL, or an invalid environment name |
| `NotSupported` | The requested operation is unavailable on this platform |
| `TimedOut` | The wait or collection deadline expired |
| `OutputLimit` | Captured output exceeded the combined limit |
| `IoFailed` | A process operation failed, or the child handle is closed |
