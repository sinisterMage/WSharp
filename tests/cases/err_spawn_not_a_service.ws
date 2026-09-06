// error: is not a service
// error: cannot be sent to another worker
// error: `counter` has no method `nope`
const geometry = @import("./modules/geometry.ws");
const bad = @import("./modules/badservice.ws");
const counter = @import("./modules/counter.ws");

fn main() i64 {
    const a = @spawn(geometry) catch return 1;
    const b = @spawn(bad, twice) catch return 1;
    const w = @spawn(counter, 1, "x") catch return 1;
    const n = w.nope() catch 0;
    return 0;
}

fn twice(x: i64) i64 { return x * 2; }
