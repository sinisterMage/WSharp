// A generic call whose type variable nothing pins.
//
// `array.new(32)` says how many elements, not what they are, and nothing here
// says either -- so there is no type to compile `main` at. Reported by
// monomorphisation, which is the pass that has to pick one.
//
// This case is also what `check_and_run_agree_about_specialisation` drives:
// `wsharp check` used to stop before monomorphisation and so accepted this,
// which made the more careful-sounding verb the more permissive one.
// error: cannot tell what type `main` is being used at
const array = @import("std/array");

fn main() i64 {
    var b = array.new(32);
    return 0;
}
