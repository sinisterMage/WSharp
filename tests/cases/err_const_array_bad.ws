// The two things a top-level `const` array may not hold, and each has its own
// reason. A `str` element is a *reference*, and the object lives in the data
// section where the collector never traces -- so a reference in one would be a
// reference nothing can see. A computed element needs somewhere to run, and
// there is no startup initialiser to run it in.
// error: a top-level `const` array may not hold `str`
// error: the collector never traces
// error: must be a `i64` literal
// error: no startup initialiser
const NAMES = []str{ "a", "b" };
const COMPUTED = []i64{ 1 + 1 };

fn main() i64 { return 0; }
