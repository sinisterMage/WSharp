// error: `@import` is only allowed at the top level
fn main() i64 {
    const http = @import("std/http");
    return 0;
}
