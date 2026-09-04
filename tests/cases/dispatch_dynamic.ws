// expect: 404 not found
// expect: 400 you sent something wrong
// expect: 200 ok
// expect: 500 something went wrong
// expect: 404 not found
// The dispatcher proper: `s` is statically a `Status`, so which overload wins
// is only decided by the runtime type id in the object's header.
const http = @import("std/http");

fn render(s: http.Status) str { return "500 something went wrong"; }
fn render(s: http.Status2xx) str { return "200 ok"; }
fn render(s: http.Status4xx) str { return "400 you sent something wrong"; }
fn render(s: http.NotFound404) str { return "404 not found"; }

fn route(s: http.Status) void { print(render(s)); }

fn main() i64 {
    route(http.NotFound404);
    route(http.Forbidden403);
    route(http.Ok200);
    route(http.ServerError500);

    // Widening to a supertype is implicit and costs nothing at run time.
    var s: http.Status = http.NotFound404;
    route(s);
    return 0;
}
