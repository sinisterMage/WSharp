// Child processes. Arguments are passed literally; no shell is involved.
const os = @import("std/os");
const array = @import("std/array");
const text = @import("std/str");

pub const Null = 0;
pub const Inherit = 1;
pub const Capture = 2;
pub const Env = struct { name: str, value: str };
pub const Command = struct {
    program: str,
    args: []str,
    cwd: str,
    env: []Env,
    clear_env: bool,
    stdin: i64,
    stdout: i64,
    stderr: i64,
    output_limit: i64,
    group: bool,
};
pub const Child = struct { handle: i64 };
pub const Status = struct { code: i64, signal: i64 };
pub const Output = struct { status: Status, stdout: str, stderr: str };

/// Capture both output streams, with a combined 8 MiB ceiling; stdin is null.
/// An empty cwd inherits the parent's directory. Environment changes are local
/// to the child. Set group=true on Unix to signal/kill its process group while
/// the leader is still owned and unreaped. Once poll/wait reaps the leader,
/// signals are no-ops: a recycled process/group ID must never be targeted.
/// Descendants that outlive that leader need their own supervision contract.
/// stdin accepts only Null and Inherit; piped input is not provided yet.
pub fn command(program: str, args: []str) Command {
    return Command{ .program = program, .args = args, .cwd = "", .env = []Env{},
        .clear_env = false, .stdin = Null, .stdout = Capture, .stderr = Capture,
        .output_limit = 8388608, .group = false };
}

/// Starts a child without waiting. Explicitly close every successful spawn.
/// Handles are process-owned, can cross workers and are never reused.
/// Capture is drained during poll/wait/collect. A child can encounter pipe
/// backpressure between calls; use Inherit or Null for unattended services.
pub fn spawn(c: Command) !{NotFound, PermissionDenied, IoFailed, BadFormat, NotSupported}Child {
    var env = []str{};
    for (c.env) |e| { env = array.push(env, e.name); env = array.push(env, e.value); }
    return Child{ .handle = try raw_spawn(c.program, os.pack(c.args), c.cwd,
        os.pack(env), c.clear_env, c.stdin, c.stdout, c.stderr, c.output_limit, c.group) };
}

/// A status is available when the direct child exits. Captured pipes may still
/// have bytes: collect waits for both EOFs. Nonzero exits are ordinary statuses.
pub fn poll(c: Child) !{IoFailed, OutputLimit}?Status {
    const result = try raw_poll(c.handle);
    if (text.len(result) == 0) { return null; }
    const fields = os.unpack(result);
    return Status{ .code = text.parse_int(fields[0]) catch -1,
        .signal = text.parse_int(fields[1]) catch 0 };
}

/// Wait for exit, draining both pipes concurrently while waiting. A negative
/// timeout waits indefinitely, zero polls. TimedOut leaves the child owned and
/// running; the caller may signal it, wait again, or close it.
pub fn wait(c: Child, timeout_ms: i64) !{IoFailed, OutputLimit, TimedOut}Status {
    const fields = os.unpack(try raw_wait(c.handle, timeout_ms, false));
    return Status{ .code = text.parse_int(fields[0]) catch -1,
        .signal = text.parse_int(fields[1]) catch 0 };
}

/// Wait for exit AND captured-stream EOF. A descendant retaining a pipe is
/// covered by the same timeout. Output is bounded across both streams; overflow
/// kills the child and reports OutputLimit, never silently truncates success.
pub fn collect(c: Child, timeout_ms: i64) !{IoFailed, OutputLimit, TimedOut}Output {
    const fields = os.unpack(try raw_wait(c.handle, timeout_ms, true));
    return Output{ .status = Status{ .code = text.parse_int(fields[0]) catch -1,
        .signal = text.parse_int(fields[1]) catch 0 }, .stdout = fields[2], .stderr = fields[3] };
}

/// Convenience capture that closes its child on every path, including timeout.
pub fn run(c: Command, timeout_ms: i64) !{NotFound, PermissionDenied, IoFailed, BadFormat, NotSupported, OutputLimit, TimedOut}Output {
    const child = try spawn(c);
    const result = collect(child, timeout_ms) catch |e| {
        close(child);
        if (e == error.TimedOut) { return error.TimedOut; }
        if (e == error.OutputLimit) { return error.OutputLimit; }
        return error.IoFailed;
    };
    close(child);
    return result;
}

/// SIGTERM on Unix. Windows has no equivalent and returns NotSupported.
/// Success means delivered (or already exited), not that shutdown completed.
pub fn terminate(c: Child) !{IoFailed, NotSupported}void { return raw_signal(c.handle, 15); }
pub fn interrupt(c: Child) !{IoFailed, NotSupported}void { return raw_signal(c.handle, 2); }
pub fn kill(c: Child) !{IoFailed, NotSupported}void { return raw_signal(c.handle, 9); }

/// Kill a still-running child, reap it and release its pipes and capture buffers.
/// Idempotent. Normal main return closes remaining children. Forced os.exit
/// and runtime panic attempt a kill without waiting on locks or reaping.
/// Like an OS wait after kill, reaping is not a hard deadline for a process stuck
/// in an uninterruptible kernel operation. No detached-child API is implicit.
pub fn close(c: Child) void { raw_close(c.handle); return; }
pub fn success(s: Status) bool { return s.code == 0 and s.signal == 0; }
