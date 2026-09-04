// expect: 15
// expect: 15
// A generic struct's instantiations are registered with the collector one at a
// time, with the pointer offsets their arguments imply -- `Box[i64]` holds no
// reference and `Box[Node]` holds one at a different offset from
// `Pair[?i64, Node]`. Getting that wrong loses objects rather than failing.
const Node = struct { value: i64 };
const Box = struct[T] { value: T };
const Pair = struct[A, B] { first: A, second: B };

fn churn(n: i64) i64 {
    var i: i64 = 0;
    var last = 0;
    while (i < n) : (i += 1) {
        const junk = Box{ .value = Node{ .value = i } };
        last = junk.value.value;
    }
    return last;
}

fn main() i64 {
    const kept = []Pair[?i64, Box[Node]]{
        Pair{ .first = null, .second = Box{ .value = Node{ .value = 1 } } },
        Pair{ .first = 2, .second = Box{ .value = Node{ .value = 2 } } },
        Pair{ .first = null, .second = Box{ .value = Node{ .value = 3 } } },
        Pair{ .first = 4, .second = Box{ .value = Node{ .value = 4 } } },
        Pair{ .first = null, .second = Box{ .value = Node{ .value = 5 } } },
    };
    var total = 0;
    for (kept) |p| { total = total + p.second.value.value; }
    print_int(total);

    churn(5000);
    gc_collect();
    gc_trace();
    gc_trace();

    var after = 0;
    for (kept) |p| { after = after + p.second.value.value; }
    print_int(after);
    return 0;
}
