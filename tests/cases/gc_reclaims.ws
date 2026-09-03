// expect: 1999
// expect: true
// expect: true
// Reference counting doing its job: two thousand objects are allocated and
// abandoned, and after collecting, almost nothing is left alive. Before the
// collector existed this program leaked every one of them.
const Node = struct { value: i64, next: ?Node };

fn churn(n: i64) i64 {
    var i: i64 = 0;
    var last = 0;
    while (i < n) : (i += 1) {
        // Dead as soon as the iteration ends: nothing on the heap or the stack
        // refers to it afterwards.
        const tmp = Node{ .value = i, .next = null };
        last = tmp.value;
    }
    return last;
}

fn main() i64 {
    print_int(churn(2000));

    // Twice: the first collection retires the objects allocated since the last
    // one, the second reclaims them. A newly allocated object is deliberately
    // exempt from the collection its own allocation triggers.
    gc_collect();
    gc_collect();

    print_bool(gc_live_objects() < 50);
    print_bool(gc_collections() > 0);
    return 0;
}
