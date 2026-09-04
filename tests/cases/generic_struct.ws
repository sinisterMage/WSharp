// A struct with type parameters. Each instantiation is laid out separately,
// because a field's width depends on what it is instantiated at: `first` is
// one slot in `Pair[i64, i64]` and two in `Pair[?i64, i64]`, which moves
// `second`.
// expect: 42
// expect: hi
// expect: 42
// expect: 2
// expect: tail
// expect: 7
// expect: 9
// expect: 5
// expect: -1
// expect: 2
const Box = struct[T] { value: T };
const Pair = struct[A, B] { first: A, second: B };

fn unwrap[T](b: Box[T]) T { return b.value; }
fn snd[A, B](p: Pair[A, B]) B { return p.second; }

fn main() i64 {
    const a = Box{ .value = 42 };
    const b = Box{ .value = "hi" };
    print_int(a.value);
    print(b.value);
    print_int(unwrap(a));

    const p = Pair{ .first = 1, .second = 2 };
    const q = Pair{ .first = Box{ .value = 7 }, .second = "tail" };
    print_int(snd(p));
    print(snd(q));
    print_int(q.first.value);

    // The annotation reaches the literal's fields, so `5` coerces into `?i64`
    // exactly as it would in any other annotated binding.
    var c: Pair[?i64, i64] = Pair{ .first = 5, .second = 9 };
    print_int(c.second);
    print_int(c.first orelse -1);
    c.first = null;
    print_int(c.first orelse -1);

    const boxes = []Box[i64]{ Box{ .value = 1 }, Box{ .value = 2 } };
    print_int(boxes[1].value);
    return 0;
}
