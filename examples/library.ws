// The standard library, and the module system it sits behind.
//
// `@import` binds a module to a name. Two files may each declare a `helper`
// without colliding, and the 27 HTTP status types no longer occupy the global
// namespace -- which is what the module system was for.

const str = @import("std/str");
const array = @import("std/array");
const math = @import("std/math");
const io = @import("std/io");
const http = @import("std/http");

fn render(s: http.Status) str { return "500 server error"; }
fn render(s: http.Status4xx) str { return "400 bad request"; }
fn render(s: http.NotFound404) str { return "404 not found"; }

// Statically a `Status`, so which overload wins is read from the type id in
// the object's header.
fn serve(s: http.Status) void { print(render(s)); }

fn main() i64 {
    // Strings. `==` compares contents, so a string built at run time equals a
    // literal.
    const greeting = str.concat("hello, ", "world");
    print(greeting);
    print(str.len(greeting));
    print(greeting == "hello, world");
    print(str.substr(greeting, 7, str.len(greeting)));

    // Splitting produces an array of strings, which `join` puts back.
    const fields = str.split("id,name,email", ",");
    print(array.len(fields));
    print(str.join(fields, " | "));

    // Arithmetic that is not an operator. `abs` is an overload set; `min` is
    // one function over the abstract type `Number`.
    print(math.abs(-7));
    print(math.sqrt(2.0));
    print(math.min(3, 7));
    print(math.ipow(2, 10));

    // I/O, where the library first has to fail: a fallible builtin returns a
    // `!T`, caught like any other error.
    print(io.read_file("/definitely/not/a/file") catch "could not read it");

    serve(http.NotFound404);
    serve(http.Forbidden403);
    serve(http.ServerError500);
    return 0;
}
