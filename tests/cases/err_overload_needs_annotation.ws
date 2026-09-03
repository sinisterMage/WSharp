// error: is an overload set
// Dispatch chooses an overload *by* its parameter types, so leaving one to
// inference would make the choice depend on the calls it is resolving.
fn size(s: Status4xx) i64 { return 4; }
fn size(s) i64 { return 0; }

fn main() i64 { return size(NotFound404); }
