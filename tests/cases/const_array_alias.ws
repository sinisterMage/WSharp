// A top-level `const` array can be written through an alias, and this case
// says so out loud rather than leaving the limitation to a paragraph.
//
// `K[0] = 1` is rejected -- `err_assign_const_array.ws` is that half. A local
// bound to `K` is an ordinary array holding the same address, and W# has no
// way to say that a reference is read-only, so the write goes through and the
// table every worker shares changes.
//
// The data is emitted *writable* for exactly this reason: a read-only page
// would turn the mistake into a fault with no message. See LIMITATIONS.md.
//
// The second half is the workaround, which is what a caller should write:
// copy first, and the copy is the thing that changes.
// expect: 99
// expect: 1
// expect: 1
// expect: 42
const array = @import("std/array");

const K = []i64{ 1, 2, 3 };

fn main() void {
    var alias = K;
    alias[0] = 99;
    print(K[0]);

    // Put it back, so the second half starts from a known table -- a top-level
    // `const` is one object for the whole process and this case has just
    // proved it can be changed.
    alias[0] = 1;
    print(K[0]);

    var copy = array.slice(K, 0, array.len(K));
    copy[0] = 42;
    print(K[0]);
    print(copy[0]);
}
