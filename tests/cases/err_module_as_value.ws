// error: `http` is a module, not a value
const http = @import("std/http");
fn main() i64 { print_int(http); return 0; }
