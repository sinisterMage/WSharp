// The HTTP status types come from the standard library.
const http = @import("std/http");
const Request = struct { path: str };

fn render(r: Request, s: http.Status)      str { return "HTTP/1.1 500 Internal Server Error"; }
fn render(r: Request, s: http.Status2xx)   str { return "HTTP/1.1 200 OK"; }
fn render(r: Request, s: http.Status4xx)   str { return "HTTP/1.1 400 Bad Request"; }
fn render(r: Request, s: http.NotFound404) str { return "HTTP/1.1 404 Not Found"; }
fn render(r: Request, s: http.Teapot418)   str { return "HTTP/1.1 418 I'm a teapot"; }

fn main() i64 {
    const req = Request{ .path = "/" };
    // Resolved at compile time: the status type is known here.
    print(render(req, http.NotFound404));
    serve(req, http.Teapot418);
    return 0;
}

// Resolved at run time, from the type id in the object's header, because
// `s` could be any status by the time this runs.
fn serve(r: Request, s: http.Status) void { print(render(r, s)); }
