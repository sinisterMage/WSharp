const str  = @import("std/str");
const http = @import("std/http");

fn main() i64 {
    for (str.split("a,b,c", ",")) |part| { print(part); }
    return 0;
}
