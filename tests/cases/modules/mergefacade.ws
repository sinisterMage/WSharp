// A facade that presents both files' `render` under one name.
//
// Two re-exports of one name merge into one overload set. A facade binds one
// name to one thing, so this used to be "declared more than once" and the way
// round it was a forwarding function per member -- which works, and says what
// `pub const render = ..` already says, twice.
const one = @import("./mergeone.ws");
const two = @import("./mergetwo.ws");

pub const Dot = one.Dot;
pub const BigDot = one.BigDot;
pub const Line = two.Line;

pub const render = one.render;
pub const render = two.render;

// The merged set is this module's own name too, so it may be used unqualified
// here -- and dispatches over every member, from both files.
pub fn describe(d: Dot) str { return render(d); }
