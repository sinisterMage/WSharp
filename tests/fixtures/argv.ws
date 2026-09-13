const os = @import("std/os");
fn main() i64 {
    for (os.args()) |arg| { print(arg); }
    return 0;
}
