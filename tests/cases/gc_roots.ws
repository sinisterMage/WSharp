// expect: 4950
// expect: 100
// A linked list built by mutation, so the list head is live across every one
// of the hundred allocations that extend it -- and across the calls that walk
// it afterwards. That is the shape the collector's roots exist for, and under
// `--gc-stress` every one of those safepoints is checked.
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

fn main() i64 {
    var head: ?Node = null;
    var i: i64 = 0;
    while (i < 100) : (i += 1) {
        // `head` is a live heap reference across this allocation.
        head = Node{ .value = i, .next = head };
    }
    print_int(sum(head));
    print_int(length(head));
    return 0;
}
