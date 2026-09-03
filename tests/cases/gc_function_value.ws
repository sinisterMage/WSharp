// expect: 42
// expect: true
// A top-level function used as a value is a heap object like any other: a
// closure with no captures, carrying the code pointer. It therefore needs a
// registered layout, or the collector cannot tell how big it is -- and
// evacuating one would copy its header and leave the code pointer behind, so
// the next call through it would jump into whatever now lives there.
const Node = struct { value: i64, next: ?Node };

fn inc(x: i64) i64 { return x + 1; }
fn apply(f: fn(i64) i64, v: i64) i64 { return f(v); }

fn churn(n: i64) i64 {
    var i: i64 = 0;
    var last = 0;
    while (i < n) : (i += 1) {
        const junk = Node{ .value = i, .next = null };
        last = junk.value;
    }
    return last;
}

fn main() i64 {
    const g = inc;
    // Leave the block holding `g` sparse, so the trace picks it to evacuate.
    churn(5000);
    gc_collect();
    gc_collect();
    gc_trace();
    gc_trace();
    print_int(apply(g, 41));
    print_bool(apply(g, 0) == 1);
    return 0;
}
