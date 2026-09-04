// Multiple dispatch over the HTTP status lattice.
//
// A response renderer written as a set of small overloads rather than a
// growing `switch`. Adding a special case for one status means adding one
// function -- nothing existing is edited, and the compiler checks that the
// new overload is unambiguously more specific than the ones it refines.
//
// The status types come from the standard library and are materialised on
// first mention; a program that never names one pays nothing for them.

const http = @import("std/http");

const Request = struct { path: str };

// The general case, then two families, then two exact codes.
fn render(r: Request, s: http.Status) str { return "HTTP/1.1 500 Internal Server Error"; }
fn render(r: Request, s: http.Status2xx) str { return "HTTP/1.1 200 OK"; }
fn render(r: Request, s: http.Status4xx) str { return "HTTP/1.1 400 Bad Request"; }
fn render(r: Request, s: http.NotFound404) str { return "HTTP/1.1 404 Not Found"; }
fn render(r: Request, s: http.Teapot418) str { return "HTTP/1.1 418 I'm a teapot"; }

// Routing decides a status at run time, so `serve` cannot know which overload
// it will need -- this is the call that compiles to a real dispatch.
fn serve(r: Request, s: http.Status) void { print(render(r, s)); }

fn main() i64 {
    const req = Request{ .path = "/" };

    // Resolved at compile time: each status here is exactly what it says.
    print(render(req, http.Ok200));
    print(render(req, http.NotFound404));
    print(render(req, http.Teapot418));
    print(render(req, http.Forbidden403));
    print(render(req, http.ServiceUnavailable503));

    // Resolved at run time, from the type id in the object's header.
    serve(req, http.NotFound404);
    serve(req, http.Created201);
    serve(req, http.BadGateway502);

    return 0;
}
