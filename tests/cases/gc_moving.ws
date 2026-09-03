// expect: 4950
// expect: 100
// expect: 4950
// expect: 100
// expect: true
// Evacuation: the collector physically relocates surviving objects out of
// sparsely occupied blocks and repoints every reference at the copy. If a
// single reference were missed, the list walk after the trace would read a
// dead object and the sums would not match.
const Node = struct { value: i64, next: ?Node };

fn sum(list: ?Node) i64 {
    var total = 0;
    var cur = list;
    var going = true;
    while (going) {
        if (cur) |node| {
            total = total + node.value;
            cur = node.next;
        } else {
            going = false;
        }
    }
    return total;
}

fn length(list: ?Node) i64 {
    var n = 0;
    var cur = list;
    var going = true;
    while (going) {
        if (cur) |node| {
            n += 1;
            cur = node.next;
        } else {
            going = false;
        }
    }
    return n;
}

fn build(n: i64) ?Node {
    var head: ?Node = null;
    var i: i64 = 0;
    while (i < n) : (i += 1) {
        head = Node{ .value = i, .next = head };
    }
    return head;
}

fn churn(n: i64) i64 {
    // Garbage interleaved with the list above, so its blocks end up mostly
    // empty once it is collected -- which is what makes them evacuation
    // candidates rather than merely reusable.
    var i: i64 = 0;
    var last = 0;
    while (i < n) : (i += 1) {
        const junk = Node{ .value = i, .next = null };
        last = junk.value;
    }
    return last;
}

fn main() i64 {
    const list = build(100);
    print_int(sum(list));
    print_int(length(list));

    churn(5000);
    gc_collect();
    gc_collect();

    // Relocate whatever survived.
    gc_trace();
    gc_trace();

    // The same list, read after every one of its nodes may have moved.
    print_int(sum(list));
    print_int(length(list));
    print_bool(gc_live_objects() >= 100);
    return 0;
}
