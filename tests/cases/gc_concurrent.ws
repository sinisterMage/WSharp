// expect: 1249975000
// expect: 50000
// expect: true
// expect: 1249975000
// expect: 50000
// expect: 100000000
// expect: 10000
// expect: 25
// expect: true
// expect: true
// The mark trace runs on its own thread while the program keeps going.
// `gc_trace_start` takes the snapshot and returns; everything between it and
// `gc_trace_finish` happens while the collector is marking: fields holding
// references are overwritten (which the write barrier must record for the
// marker), new objects are allocated and pointed at old ones, garbage is
// made, and cycles created before the snapshot are left to be reclaimed. If
// the marker missed anything, the sums afterwards would be wrong or the
// program would read freed memory.
const Node = struct { value: i64, next: ?Node };

fn build(n: i64) ?Node {
    var head: ?Node = null;
    var i: i64 = 0;
    while (i < n) : (i += 1) {
        head = Node{ .value = i, .next = head };
    }
    return head;
}

fn sum(list: ?Node) i64 {
    var total = 0;
    var cur = list;
    while (cur) |node| {
        total = total + node.value;
        cur = node.next;
    }
    return total;
}

fn length(list: ?Node) i64 {
    var n = 0;
    var cur = list;
    while (cur) |node| {
        n += 1;
        cur = node.next;
    }
    return n;
}

// Unlink every other node, overwriting a reference field each time.
fn thin(list: ?Node) void {
    var cur = list;
    while (cur) |node| {
        if (node.next) |skipped| {
            node.next = skipped.next;
        }
        cur = node.next;
    }
}

fn make_cycles(n: i64) void {
    var i: i64 = 0;
    while (i < n) : (i += 1) {
        var a = Node{ .value = 1, .next = null };
        const b = Node{ .value = 2, .next = a };
        a.next = b;
    }
}

fn churn(n: i64) i64 {
    var i: i64 = 0;
    var last = 0;
    while (i < n) : (i += 1) {
        const junk = Node{ .value = i, .next = null };
        last = junk.value;
    }
    return last;
}

fn main() i64 {
    const keep = build(50000);
    const list = build(20000);
    print_int(sum(keep));
    print_int(length(keep));

    make_cycles(1000);
    gc_collect();
    gc_collect();
    const before = gc_live_objects();
    print_bool(before >= 72000);

    gc_trace_start();

    // While the collector thread is marking:
    thin(list);
    var bridge = Node{ .value = 100, .next = keep };
    const bridge2 = Node{ .value = 200, .next = bridge };
    bridge = Node{ .value = 300, .next = list };
    churn(30000);
    gc_collect();

    gc_trace_finish();

    print_int(sum(keep));
    print_int(length(keep));
    print_int(sum(list));
    print_int(length(list));
    // The chain through the objects made during the mark still stands.
    var via = 0;
    if (bridge2.next) |b| { via = via + b.value; }
    if (bridge.next) |l| { via = via + l.value - 19999; }
    print_int(via / 4);
    // The cycles, unreachable since before the snapshot, are gone.
    print_bool(gc_live_objects() < before);
    print_bool(gc_traces() >= 1);
    return 0;
}
