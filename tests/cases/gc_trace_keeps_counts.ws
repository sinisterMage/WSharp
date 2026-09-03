// expect: 2
// expect: 3
// expect: 2
// expect: 1
// A mark trace must not discard the write barrier's pending work. `b` is
// stored into a field after the last counting collection, so the increment
// for it is still sitting in the barrier's buffer when the trace runs; if the
// trace threw that buffer away, `a` would be one reference short and the next
// collections would free it out from under `b.next`.
const Node = struct { value: i64, next: ?Node };

fn main() i64 {
    gc_collect();
    var a = Node{ .value = 1, .next = null };
    const b = Node{ .value = 2, .next = a };
    print_int(gc_live_objects());

    gc_trace();

    // The only reference left to the first node is `b.next`.
    a = Node{ .value = 99, .next = null };
    print_int(gc_live_objects());
    gc_collect();
    gc_collect();
    gc_collect();
    // `b` and the node it points at; the replacement is dead.
    print_int(gc_live_objects());
    if (b.next) |n| { print_int(n.value); }
    return 0;
}
