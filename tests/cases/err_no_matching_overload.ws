// error: no overload of `render` accepts
// Both overloads take a status; a string is not one, so there is nothing to
// dispatch to.
fn render(s: Status4xx) i64 { return 4; }
fn render(s: Status5xx) i64 { return 5; }

fn main() i64 { return render("not a status"); }
