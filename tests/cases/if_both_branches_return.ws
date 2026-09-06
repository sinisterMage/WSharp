// An `if` whose arms both leave, with nothing after it.
//
// This used to fail to compile with `Verifier errors` and no more detail, and
// the reason is worth writing down because the comment that caused it was
// almost right. Code generation creates a merge block for every `if` and
// switches to it afterwards; the function epilogue then closes whatever block
// is open with a valueless `return`, which is correct "only for a `void`
// function -- inference rejects anything else". But inference rejects a
// function that can *fall through*, and this one cannot: both arms return, so
// the merge block is unreachable rather than reachable-and-void. The epilogue
// closed it with a `return` carrying no value, in a function whose signature
// says it returns a `str`, and Cranelift refused the result.
//
// The fix is to switch to the merge only when an arm can reach it. It shows up
// the moment anything is written with an if-chain, which is what a protocol
// state machine is made of, so `std/tls` met it immediately.
// expect: one
// expect: two
// expect: many
// expect: ok
// expect: 7

fn pick(n: i64) str {
    if (n == 1) { return "one"; }
    else if (n == 2) { return "two"; }
    else { return "many"; }
}

/// The same shape with no value to return, which always worked -- the
/// epilogue's valueless `return` happened to match.
fn shout(n: i64) void {
    if (n > 0) { print("ok"); return; }
    else { print("not ok"); return; }
}

/// And with a value produced after the `if`, which is the case that kept the
/// merge block reachable and so kept the bug hidden.
fn after(n: i64) i64 {
    if (n == 0) { return 7; }
    else { }
    return 8;
}

fn main() i64 {
    print(pick(1));
    print(pick(2));
    print(pick(9));
    shout(1);
    print_int(after(0));
    return 0;
}
