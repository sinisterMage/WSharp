// A generic `fn` literal that captures a heap reference, across a whole
// collection cycle.
//
// Each instantiation is a separate function with a closure object of its own,
// built at the use site rather than at the binding, and the captured value is
// snapshotted into a hidden local that the definition's statement introduces.
// That local is a root like any other: if the code generator failed to declare
// it, the reference would go stale the moment the trace moved the node it
// names, and the sums below would disagree.
// expect: 7
// expect: seven
// expect: 7
// expect: seven
// expect: 7
// expect: true
const Node = struct { value: i64 };

fn churn(n: i64) i64 {
    var i: i64 = 0;
    var last = 0;
    while (i < n) : (i += 1) {
        const junk = Node{ .value = i };
        last = junk.value;
    }
    return last;
}

fn main() i64 {
    const kept = Node{ .value = 7 };
    const words = []str{ "seven", "eight" };

    // Two captures of two different heap types, shared by every instantiation
    // because their types belong to this frame and not to the literal.
    const at = fn [T](a: []T, i: i64) T {
        print_int(kept.value);
        return a[i];
    };
    print(at(words, 0));

    // Leave the blocks holding all of it sparse enough to be evacuated.
    churn(5000);
    gc_collect();
    gc_trace();
    gc_trace();

    // Every reference the definition holds must still name what it named.
    print(at(words, 0));
    print_bool(kept.value == 7 and at([]i64{ 1, 2 }, 1) == 2);
    return 0;
}
