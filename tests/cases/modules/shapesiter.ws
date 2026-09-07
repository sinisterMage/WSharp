// A second file of the same package, so the facade has more than one thing to
// gather -- which is the situation re-export exists for.
const inner = @import("./shapesinner.ws");

pub const Walk = struct { at: i64, stop: i64 };

// `iter` and `next` are resolved in the module that declares the *type*, never
// through the name a caller reached it by. So these have to be `pub` here, and
// a facade that re-exported `Walk` could not have re-homed them.
pub fn iter(w: Walk) Walk { return Walk{ .at = w.at, .stop = w.stop }; }
pub fn next(w: Walk) ?i64 {
    if (w.at >= w.stop) { return null; }
    const v = w.at;
    w.at += 1;
    return v;
}

pub fn to(n: i64) Walk { return Walk{ .at = 0, .stop = n }; }

pub fn scaled(p: inner.Point) i64 { return inner.norm2(p) * inner.SCALE; }
