// error: never produces a value
fn risky(n: i64) !i64 { return n; }
fn main() i64 {
    return risky(1) catch { print("oh"); };
}
