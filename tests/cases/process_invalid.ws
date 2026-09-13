// expect: invalid commands rejected
const p = @import("std/process");
fn main() i64 {
    var c = p.command("", []str{});
    const a = p.spawn(c) catch |e| {
        if (e != error.BadFormat) { return 1; }
        return bad_mode();
    };
    p.close(a); return 2;
}
fn bad_mode() i64 {
    var c = p.command("unused", []str{}); c.stdin = p.Capture;
    const a = p.spawn(c) catch |e| {
        if (e != error.BadFormat) { return 3; }
        print("invalid commands rejected"); return 0;
    };
    p.close(a); return 4;
}
