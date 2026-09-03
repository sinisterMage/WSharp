// expect: 2999
// expect: 4498500
// expect: true
// A block that was emptied and recycled still holds its previous occupants'
// bytes unless it is zeroed. The first pass fills several blocks with objects
// whose word at the offset of `Node.next` is an integer that is not a pointer;
// the second pass allocates nodes from the recycled blocks. Under `--gc-stress`
// a collection runs between each allocation and its initialising stores, so
// the barrier reads the new object's fields -- which must read as null.
const Wide = struct { x: i64, y: i64, z: i64 };
const Node = struct { value: i64, next: ?Node };

fn fill_wide(n: i64) i64 {
    var i: i64 = 0;
    var last = 0;
    while (i < n) : (i += 1) {
        const w = Wide{ .x = i, .y = 1, .z = 1000000 };
        last = w.x;
    }
    return last;
}

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

fn build(n: i64) ?Node {
    var head: ?Node = null;
    var i: i64 = 0;
    while (i < n) : (i += 1) {
        head = Node{ .value = i, .next = head };
    }
    return head;
}

fn main() i64 {
    print_int(fill_wide(3000));
    gc_collect();
    gc_collect();
    gc_collect();
    const list = build(3000);
    gc_collect();
    gc_collect();
    print_int(sum(list));
    print_bool(gc_live_objects() >= 3000);
    return 0;
}
