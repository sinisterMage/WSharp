// Evacuation with a graph the write barrier keeps logging: the one shape the
// collector's other cases did not have, and the one that found a real bug.
//
// The lists the evacuation pause reads -- the slots the marker saw pointing
// into a block being emptied, and the objects the trace touched afterwards --
// are recorded during the *mark*. Counting's frees used to run at the pause
// that finishes marking, which is before the pause that reads those lists: a
// logged object freed in between had its space taken by something else while
// objects moved, and the pause then walked a stranger's bytes with a dead
// object's layout. The frees now wait one pause longer.
//
// Reproducing that needs volume rather than cleverness -- a few hundred
// thousand short-lived nodes, a handful of scattered survivors to make blocks
// sparse enough to evacuate, and a field overwritten on every pass so the
// barrier keeps logging. It is what gives `--gc-stress`'s check something to
// fire on; the check is what makes the answer definite.
// expect: 150
// expect: true
// expect: true
const list = @import("std/list");

/// A node with two references, so the heap holds a graph rather than a chain.
const Node = struct { left: ?Node, right: ?Node, tag: i64 };

fn leaf(n: i64) Node { return Node{ .left = null, .right = null, .tag = n }; }
fn pair(a: Node, b: Node, n: i64) Node { return Node{ .left = a, .right = b, .tag = n }; }

fn total(n: Node) i64 {
    var sum = n.tag;
    if (n.left) |l| { sum += total(l); }
    if (n.right) |r| { sum += total(r); }
    return sum;
}

fn main() i64 {
    // Survivors kept every thousandth pass, scattered through the heap: a
    // block pinned by a handful of them is exactly what evacuation is for.
    var kept: list.List[Node] = list.new();
    var sum = 0;
    var i = 0;
    while (i < 150000) : (i += 1) {
        const t = pair(pair(leaf(i), leaf(i + 1), i), leaf(i + 2), i);
        sum += total(t);
        // Overwriting a survivor's field is what logs it, and the logged list
        // is one of the things the evacuation pause revisits.
        if (list.len(kept) > 0) {
            const old = list.get(kept, list.len(kept) - 1);
            old.left = t;
        }
        if (i % 1000 == 0) { list.push(kept, t); }
    }
    // Everything held is still readable, which is the point: a survivor whose
    // fields were fixed up wrongly would not be.
    var check = 0;
    for (list.to_array(kept)) |n| { check += total(n); }
    print_int(list.len(kept));
    print_bool(check > 0);
    print_bool(sum > 0);
    return 0;
}
