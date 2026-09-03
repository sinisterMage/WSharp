// expect: 3199960000
// expect: 5000
// expect: true
// Enough allocation for the collector to start traces on its own, while a
// live structure keeps changing under the marker. Nothing here calls the
// collector explicitly; the traces begin at allocation safepoints and finish
// at loop back edges or later allocations, whichever comes first.
const Node = struct { value: i64, next: ?Node };

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

// A ring buffer of lists: the newest list is kept, the one five rounds old
// is dropped, and the tail of each list is rewritten as it is built.
fn main() i64 {
    var a: ?Node = null;
    var b: ?Node = null;
    var c: ?Node = null;
    var d: ?Node = null;
    var e: ?Node = null;
    var total = 0;
    var round = 0;
    while (round < 16) : (round += 1) {
        var head: ?Node = null;
        var i: i64 = 0;
        while (i < 5000) : (i += 1) {
            head = Node{ .value = round * 5000 + i, .next = head };
            // Every so often, cut the list in two and rejoin it, so that
            // reference fields are overwritten while a trace may be marking.
            if (i % 1000 == 999) {
                if (head) |h| {
                    if (h.next) |n| {
                        h.next = n.next;
                        head = Node{ .value = n.value, .next = h };
                    }
                }
            }
        }
        total = total + sum(head);
        e = d;
        d = c;
        c = b;
        b = a;
        a = head;
    }
    print_int(total);
    print_int(length(a));
    print_bool(gc_traces() >= 2);
    return 0;
}
