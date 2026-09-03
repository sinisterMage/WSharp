// expect: first
// expect: second
// expect: 2
// Overwriting a field that holds a heap reference is what the write barrier
// exists for: the old value loses a reference and the new one gains it.
// Assigning an `i64` field, by contrast, changes no one's count and must emit
// no barrier at all.
const Cell = struct { label: str, count: i64 };
const Holder = struct { cell: Cell };

fn main() i64 {
    const first = Cell{ .label = "first", .count = 1 };
    const second = Cell{ .label = "second", .count = 2 };

    var h = Holder{ .cell = first };
    print(h.cell.label);

    // A heap reference is overwritten here: barrier.
    h.cell = second;
    print(h.cell.label);

    // A scalar is overwritten here: no barrier.
    h.cell.count = 2;
    print_int(h.cell.count);
    return 0;
}
