// A module with a surface and an inside. Everything here is private unless it
// says `pub`, which is what makes a module's surface something it states
// rather than something it leaks.
pub const Counter = struct { n: i64 };

// The inside: a caller has no business knowing how the step is chosen.
const STEP = 3;
fn stepped(n: i64) i64 { return n + STEP; }

pub fn start() Counter { return Counter{ .n = 0 }; }
pub fn bump(c: Counter) i64 {
    c.n = stepped(c.n);
    return c.n;
}

// Two modules may each have a private `helper`, exactly as before -- privacy
// only adds a rule about who may name one, not about what may be declared.
fn helper() i64 { return 100; }
pub fn call_helper() i64 { return helper(); }
