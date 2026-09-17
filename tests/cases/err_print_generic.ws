// error: `print` accepts a string, integer, float or boolean

fn show(value) void { print(value); }
fn forward(value) void { show(value); }

fn main() void { forward([]i64{ 1, 2 }); }
