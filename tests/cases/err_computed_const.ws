// A top-level `const` may be bound to a literal or a `fn`, and to nothing
// else. This is the guard for that, and it exists because the restriction is a
// *documented limitation* rather than an oversight: LIMITATIONS.md, "A computed
// top-level `const` is rejected".
//
// The reason is the collector's, not the parser's. A computed global needs
// storage that outlives every function and a startup initialiser to fill it,
// and the moment such storage exists the collector has to treat it as a root --
// a fifth root list beside `gc::collect`'s three and `worker::PINNED`, and
// every root list has to be added to four places at once (the collect root set,
// the evacuation pause's root pass, `evacuate::fix_references` and the
// `--gc-stress` verifier). That is the size of the fix, and it is why the
// diagnostic says what to do instead rather than apologising.
//
// Both spellings are here because they fail for the same reason and a reader
// meeting one will try the other: arithmetic, and a call. `err_const_array_bad.ws`
// is the neighbouring case, for a computed *element* of a `const` array.
// error: a top-level `const` must be a literal or a `fn`
// error: computed globals need a startup initialiser
const N = 2 + 3;
const M = double(2);

fn double(x: i64) i64 { return x * 2; }

fn main() i64 { return 0; }
