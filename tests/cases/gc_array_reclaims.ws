// expect: true
// expect: true
// An array holds its elements alive, and releasing it releases them: the
// barrier has to see element slots for the counts to come out right.
const Node = struct { value: i64 };

fn build(n: i64) []Node {
    var a = []Node{ Node{ .value = 0 }, Node{ .value = 0 }, Node{ .value = 0 } };
    var i: i64 = 0;
    while (i < 3) : (i += 1) { a[i] = Node{ .value = n + i }; }
    return a;
}

fn main() i64 {
    const before = gc_live_objects();
    var j: i64 = 0;
    while (j < 200) : (j += 1) {
        const junk = build(j);
        if (junk[0].value < 0) { print("never"); }
    }
    gc_collect();
    gc_trace();
    // 200 arrays of 3 nodes each were made and dropped; almost nothing of it
    // should still be live.
    print_bool(gc_live_objects() < before + 50);

    const kept = build(100);
    gc_collect();
    print_bool(kept[2].value == 102);
    return 0;
}
