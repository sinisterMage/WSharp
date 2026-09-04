// A fixture, imported by `module_import.ws`. It lives in a subdirectory
// because the harness runs every `.ws` directly in `tests/cases`, and this one
// has no `main` of its own.
const Point = struct { x: i64, y: i64 };

fn norm2(p: Point) i64 { return p.x * p.x + p.y * p.y; }

// The same name as one in the importing file, which is the point: a module is
// a namespace, so the two do not collide.
fn helper() i64 { return 7; }
