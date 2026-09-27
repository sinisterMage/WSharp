// #47 workaround: a function-local array can hold traced references.
// expect: .json
fn main() void {
    const suffixes = []str{ ".json" };
    for (suffixes) |suffix| { print(suffix); }
}
