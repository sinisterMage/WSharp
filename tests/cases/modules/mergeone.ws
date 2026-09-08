// One file of a package, with an overload set of its own.
//
// `render` is a name this package presents; so is the `render` in
// `mergetwo.ws`. Neither file knows about the other.
pub const Dot = struct { };
pub const BigDot = struct : Dot { };

pub fn render(d: Dot) str { return "dot"; }
pub fn render(d: BigDot) str { return "big dot"; }
