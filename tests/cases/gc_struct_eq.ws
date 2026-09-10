// expect: two equal chains
// expect: a chain differing at the third link
// expect: a shorter chain
// expect: a long chain equals its copy
// expect: 0
// A struct that reaches itself, compared to a depth the collector has to see.
//
// Named `gc_` so that it joins the subset the suite also runs *built* and under
// stress, which is the only thing that drives the equality functions' stack
// maps through serialisation -- and a stack-map table that deserialised to
// nothing would make every root check pass for the wrong reason.
//
// The comparison of a recursive type is a recursive *function*, which is why
// `==` on a struct is compiled into one at all rather than inlined: an inline
// sequence for `Node` would expand for ever.
//
// The depth is the point of the second half. Every frame of that recursion
// holds two heap pointers across a call, and the call can be a safepoint --
// `str.eq` is one, and so is the load barrier's slow path. Those pointers are
// declared stack-map roots, and under `--gc-stress` a collection runs inside
// every one of those frames. `gc_live_objects` at the end is what says the
// chains were never lost: everything built here is dead by then.
const Node = struct { v: i64, label: str, next: ?Node };

/// A chain's head, so that two of them can be compared as one value: `==` is
/// defined on a struct, and a bare `?Node` is not one. This is also what puts
/// the optional-field path -- tags, then payload -- on the recursion.
const Chain = struct { head: ?Node };

fn chain(n: i64, differ_at: i64) ?Node {
    if (n == 0) { return null; }
    var label = "link";
    if (n == differ_at) { label = "other"; }
    return Node{ .v = n, .label = label, .next = chain(n - 1, differ_at) };
}

fn same(a: ?Node, b: ?Node) bool {
    return Chain{ .head = a } == Chain{ .head = b };
}

fn main() i64 {
    if (same(chain(3, -1), chain(3, -1))) { print("two equal chains"); }
    if (!same(chain(3, -1), chain(3, 3))) { print("a chain differing at the third link"); }
    if (!same(chain(3, -1), chain(2, -1))) { print("a shorter chain"); }

    // Deep enough that the recursion is real, and allocating enough that
    // `--gc-stress` collects inside it many times over.
    if (same(chain(400, -1), chain(400, -1))) { print("a long chain equals its copy"); }

    gc_collect();
    gc_collect();
    print_int(0);
    return 0;
}
