// A facade may only gather what it is allowed to see. Renaming is not a way
// round `pub`: the name is checked where it is written, exactly as
// `vis.helper()` is.
// error: `secret` is private to the module that declares it
const inner = @import("./modules/shapesinner.ws");

pub const secret = inner.secret;

fn main() i64 { return 0; }
