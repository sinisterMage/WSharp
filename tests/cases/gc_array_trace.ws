// expect: 15
// expect: 15
// expect: true
// Arrays through a whole collection cycle. `TypeLayout.ptr_offsets` is a fixed
// list and cannot describe an array's elements, so the collector reads a
// stride and the count in the object's header instead. Three levels of
// reference here -- an array of arrays of structs -- are all element slots,
// and a single one missed by the barrier, the marker or the evacuation fix-up
// would make the second sum disagree with the first.
const Node = struct { value: i64 };

fn one(n: i64) []Node {
    var a = []Node{ Node{ .value = 0 } };
    a[0] = Node{ .value = n };
    return a;
}

fn churn(n: i64) i64 {
    // Garbage interleaved with the arrays above, so their blocks end up
    // sparse enough to be chosen for evacuation rather than merely reused.
    var i: i64 = 0;
    var last = 0;
    while (i < n) : (i += 1) {
        const junk = Node{ .value = i };
        last = junk.value;
    }
    return last;
}

fn total(rows: [][]Node) i64 {
    var t = 0;
    var i: i64 = 0;
    while (i < 5) : (i += 1) { t = t + rows[i][0].value; }
    return t;
}

fn main() i64 {
    const kept = [][]Node{ one(1), one(2), one(3), one(4), one(5) };
    print_int(total(kept));

    churn(5000);
    gc_collect();
    // Relocate whatever survived, then read it all again.
    gc_trace();
    gc_trace();
    print_int(total(kept));
    print_bool(gc_live_objects() >= 11);
    return 0;
}
