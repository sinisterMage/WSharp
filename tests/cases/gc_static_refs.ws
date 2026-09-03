// expect: a
// expect: b
// expect: c
// A field can hold a string literal, which lives in the JIT's read-only data
// section rather than the heap. Overwriting such a field logs the old value,
// and the collector must recognise it as immortal rather than try to adjust a
// count in memory it cannot write.
const Cell = struct { label: str };

fn main() i64 {
    var c = Cell{ .label = "a" };
    print(c.label);
    gc_collect();
    c.label = "b";
    print(c.label);
    gc_collect();
    c.label = "c";
    gc_collect();
    gc_trace();
    print(c.label);
    return 0;
}
