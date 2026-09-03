// expect: 7
// expect: true
// expect: true
// A reference cycle: two nodes pointing at each other. Each is referenced
// once, so neither count ever reaches zero and reference counting alone can
// never free them -- this is precisely the case the backup mark trace exists
// for, and the reason LXR is not just a counting collector.
const Node = struct { value: i64, next: ?Node };

fn make_cycle(n: i64) i64 {
    var i: i64 = 0;
    while (i < n) : (i += 1) {
        var a = Node{ .value = 1, .next = null };
        var b = Node{ .value = 2, .next = a };
        // Closing the loop. Nothing outside will refer to either node once
        // this function returns.
        a.next = b;
    }
    return n;
}

fn main() i64 {
    print_int(make_cycle(7));

    // Counting cannot touch them.
    gc_collect();
    gc_collect();
    const after_counting = gc_live_objects();

    // Reachability can.
    gc_trace();
    const after_tracing = gc_live_objects();

    print_bool(after_counting >= 14);
    print_bool(after_tracing < after_counting);
    return 0;
}
