// A local binding shadows an imported module, so adding an import cannot
// break code that already uses the name.
// expect: 5
// expect: 404
const http = @import("std/http");
fn render(s: http.NotFound404) str { return "404"; }
fn main() i64 {
    const http = 5;
    print_int(http);
    // The type annotation is resolved where it is written, which is outside
    // the local's scope, so the module is still reachable there.
    print(render_it());
    return 0;
}
fn render_it() str { return render(http.NotFound404); }
