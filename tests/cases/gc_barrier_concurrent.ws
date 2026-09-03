// expect: 354000
// expect: 354000
// expect: 1770
// expect: true
// Both barriers under load while the collector is actually moving objects.
//
// The two lists are small enough to sit in a handful of lines, and the churn
// after them fills the rest of their blocks with garbage; collecting that
// leaves those blocks nearly empty, which is what makes them evacuation
// candidates rather than merely reusable. Between `gc_trace_start` and
// `gc_trace_finish` the program then walks the lists over and over. Every
// `node.next` is a reference read out of a block being emptied, so it takes
// the load barrier's slow path and the program moves the node itself; every
// `node.tag` write takes the write barrier's slow path, which has to record
// what it overwrote without tripping over a header that is now a forwarding
// address. If any of that were wrong the sums would not come out.
//
// A run should report objects moved (`WSHARP_GC_STATS=1`); if it reports none,
// the window this exists to test is not being entered at all.
const Node = struct { value: i64, next: ?Node, tag: str };

fn build(n: i64) ?Node {
    var head: ?Node = null;
    var i: i64 = 0;
    while (i < n) : (i += 1) {
        head = Node{ .value = i, .next = head, .tag = "a" };
    }
    return head;
}

fn garbage(n: i64) i64 {
    var i: i64 = 0;
    var last = 0;
    while (i < n) : (i += 1) {
        const junk = Node{ .value = i, .next = null, .tag = "junk" };
        last = junk.value;
    }
    return last;
}

fn churn(list: ?Node, rounds: i64) i64 {
    var r: i64 = 0;
    var total = 0;
    while (r < rounds) : (r += 1) {
        var cur = list;
        while (cur) |node| {
            // A string literal: immortal, and so never counted or moved.
            node.tag = "b";
            // Rewriting a field with what it already held still logs it.
            if (node.next) |nxt| {
                node.next = nxt;
            }
            total = total + node.value;
            cur = node.next;
        }
    }
    return total;
}

fn main() i64 {
    // Small enough that both lists together occupy well under a quarter of
    // a block's lines, which is the test a block has to fail to be worth
    // evacuating. Larger lists simply fill their block and are left alone.
    const a = build(60);
    const b = build(60);
    // Fill the rest of their blocks, then take it away again.
    garbage(4000);
    gc_collect();
    gc_collect();

    gc_trace_start();
    print_int(churn(a, 200));
    print_int(churn(b, 200));
    gc_trace_finish();

    // Both lists are intact after every node in them has moved.
    print_int(churn(a, 1));
    print_bool(churn(b, 1) == 1770);
    return 0;
}
