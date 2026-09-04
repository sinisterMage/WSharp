// A program made of several files. `@import` binds a module to a name, and
// everything in it is reached through that name -- so two files may each
// declare a `helper` without colliding, which is what the flat namespace could
// not do.
// expect: 1
// expect: 7
// expect: 25
// expect: 0
// expect: 41
// expect: 404 not found
// expect: 400 bad request
// expect: 404 not found
const geometry = @import("./modules/geometry.ws");
const wrapper = @import("./modules/wrapper.ws");
const http = @import("std/http");

fn helper() i64 { return 1; }

fn render(s: http.Status) str { return "500 server error"; }
fn render(s: http.Status4xx) str { return "400 bad request"; }
fn render(s: http.NotFound404) str { return "404 not found"; }

// Dispatch reaches across modules: the lattice is the standard library's and
// the overloads are this file's.
fn serve(s: http.Status) void { print(render(s)); }

fn main() i64 {
    print_int(helper());
    print_int(geometry.helper());

    const p = geometry.Point{ .x = 3, .y = 4 };
    print_int(geometry.norm2(p));

    // A module may import another; `wrapper` reaches `geometry` itself.
    print_int(wrapper.origin().x);
    print_int(wrapper.wrap(41).value);

    print(render(http.NotFound404));
    serve(http.Forbidden403);
    serve(http.NotFound404);
    return 0;
}
