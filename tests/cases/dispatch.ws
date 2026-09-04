// expect: 404 not found
// expect: 400 you sent something wrong
// expect: 200 ok
// expect: 500 something went wrong
// Multiple dispatch over the standard library's HTTP status lattice. Nothing
// is declared: the status types are materialised on first mention.
//
// Every call here is resolved statically, because each argument's type is
// already the exact type it will be at run time.
const http = @import("std/http");

fn render(s: http.Status) str { return "500 something went wrong"; }
fn render(s: http.Status2xx) str { return "200 ok"; }
fn render(s: http.Status4xx) str { return "400 you sent something wrong"; }
fn render(s: http.NotFound404) str { return "404 not found"; }

fn main() i64 {
    print(render(http.NotFound404));
    print(render(http.Forbidden403));
    print(render(http.Ok200));
    print(render(http.ServerError500));
    return 0;
}
