// expect: nested values still compare
// expect: true
// expect: false
// panic: cannot be compared
// A struct that reaches itself is refused by `==`, with a diagnostic.
//
// `==` on a struct compares its fields and recurses into a struct field, so a
// value that reaches itself -- a parent pointer, a doubly linked list, a graph
// node -- used to recurse until the process ran its stack out and died on a
// signal. That is a crash with no W# diagnostic from a program using no FFI,
// which the release criteria call a P1 whatever the documentation says about
// it. The comparison counts how deep it is instead, and panics past
// `EQ_MAX_DEPTH`.
//
// Both halves are asserted here, because a bound that refused ordinary values
// would be a worse defect than the one it fixed:
//
//  * a list 64 long compares by recursing 64 deep, and still answers; and
//  * a node whose `next` is itself panics with a message naming the rule,
//    rather than aborting.
//
// The `false` is the half that is easy to lose: a deep comparison must still
// be able to answer *unequal*, which walks the whole chain to the last node
// rather than stopping at the first field.
const Node = struct { next: ?Node, v: i64 };

/// A chain of `n` nodes carrying `0 .. n`, with `last` at the end of it.
fn chain(n: i64, last: i64) Node {
    var head = Node{ .next = null, .v = last };
    var i = 0;
    while (i < n) : (i += 1) {
        head = Node{ .next = head, .v = i };
    }
    return head;
}

fn main() i64 {
    print("nested values still compare");
    print(chain(64, 7) == chain(64, 7));
    // Differs only in the very last node, so the comparison has to recurse the
    // whole way to tell.
    print(chain(64, 7) == chain(64, 8));

    var loop = Node{ .next = null, .v = 1 };
    loop.next = loop;
    print(loop == loop);
    return 0;
}
