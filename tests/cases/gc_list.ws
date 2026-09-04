// A list of heap objects across several growths and a whole collection cycle.
// Every growth allocates a new backing array and copies references into it, so
// this is the case that would fail if the copy skipped the load barrier: the
// references would name objects in blocks about to be released. It is also the
// reason `std/list` is written in W# rather than Rust.
// expect: 4950
// expect: 4950
// expect: true
const list = @import("std/list");
const Node = struct { value: i64 };

fn churn(n: i64) i64 {
    // Garbage between the nodes, so their blocks end up sparse enough to be
    // chosen for evacuation rather than merely reused.
    var i: i64 = 0;
    var last = 0;
    while (i < n) : (i += 1) {
        const junk = Node{ .value = i };
        last = junk.value;
    }
    return last;
}

fn total(l: list.List[Node]) i64 {
    var t = 0;
    var i: i64 = 0;
    while (i < list.len(l)) : (i += 1) { t = t + list.get(l, i).value; }
    return t;
}

fn main() i64 {
    var kept: list.List[Node] = list.new();
    var i: i64 = 0;
    // A hundred pushes from empty is five reallocations, each one copying
    // every reference the list holds.
    while (i < 100) : (i += 1) {
        list.push(kept, Node{ .value = i });
        churn(20);
    }
    print_int(total(kept));

    gc_collect();
    // Relocate whatever survived, then read every element again.
    gc_trace();
    gc_trace();
    print_int(total(kept));
    print_bool(gc_live_objects() >= 101);
    return 0;
}
