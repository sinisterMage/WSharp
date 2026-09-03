// expect: 404 not found
// expect: 400 you sent something wrong
// expect: 200 ok
// expect: 500 something went wrong
// expect: 404 not found
// The dispatcher proper: `s` is statically a `Status`, so which overload wins
// is only decided by the runtime type id in the object's header.
fn render(s: Status) str { return "500 something went wrong"; }
fn render(s: Status2xx) str { return "200 ok"; }
fn render(s: Status4xx) str { return "400 you sent something wrong"; }
fn render(s: NotFound404) str { return "404 not found"; }

fn route(s: Status) void { print(render(s)); }

fn main() i64 {
    route(NotFound404);
    route(Forbidden403);
    route(Ok200);
    route(ServerError500);

    // Widening to a supertype is implicit and costs nothing at run time.
    var s: Status = NotFound404;
    route(s);
    return 0;
}
