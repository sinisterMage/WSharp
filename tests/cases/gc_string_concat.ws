// expect: 400
// expect: 50
// expect: 49
// expect: true
// expect: 400
// expect: 49
// The first builtins that allocate, and so the first that are safepoints.
// Building strings and arrays of strings in a loop means every iteration can
// collect, move what earlier iterations made, and finish a trace.
//
// This is why everything that moves a reference from one object into another
// is written in W# rather than Rust: generated code goes through the write
// barrier, the load barrier and the stack maps, and a runtime function would
// have to reproduce all three.
const str = @import("std/str");
const array = @import("std/array");

fn main() i64 {
    var acc = "";
    var i: i64 = 0;
    while (i < 200) : (i += 1) { acc = str.concat(acc, "ab"); }
    print_int(str.len(acc));

    var parts = []str{};
    i = 0;
    while (i < 50) : (i += 1) { parts = array.push(parts, str.from_int(i)); }
    print_int(array.len(parts));
    print(parts[49]);

    // Garbage interleaved with the strings above, so their blocks end up
    // sparse enough to be chosen for evacuation rather than merely reused.
    var junk = "";
    i = 0;
    while (i < 500) : (i += 1) { junk = str.from_int(i); }
    print_bool(str.len(junk) > 0);

    gc_collect();
    gc_trace();
    gc_trace();

    // Everything built above, read after it may all have moved.
    print_int(str.len(acc));
    print(parts[49]);
    return 0;
}
