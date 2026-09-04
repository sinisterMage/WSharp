// error: cannot find `nope` in this scope
// A path whose first segment names nothing is reported as the unknown name it
// is, rather than as a module: `nope` might have been meant as either.
fn main() i64 { return nope.thing(); }
