const process = @import("std/process");

fn main() i64 {
    const cmd = process.command("git", []str{ "--version" });
    const output = process.run(cmd, 5000) catch return 1;
    if (!process.success(output.status)) {
        print(output.stderr);
        return 1;
    }
    print(output.stdout);
    return 0;
}
