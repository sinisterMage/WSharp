// A byte array is a heap object with a stride of one, which is the shape a
// string already had. The collector reaches it through the same `elem_stride`
// it uses for `str`, so what this really checks is that packing did not teach
// the heap walker a wrong size: an object whose claimed size disagrees with
// its real one loses the walker's place at the first gap.
// expect: 4096
// expect: 255
// expect: 1
// expect: reclaimed
const array = @import("std/array");

fn fill(n: i64) []u8 {
    const bytes: []u8 = array.new(n);
    var i = 0;
    while (i < n) : (i += 1) { bytes[i] = u8(i); }
    return bytes;
}

fn churn(rounds: i64) void {
    var i = 0;
    while (i < rounds) : (i += 1) {
        const scratch = fill(512);
        // Read something back so the array cannot be optimised away.
        if (scratch[7] != 7) { print("wrong byte"); }
    }
}

fn main() i64 {
    const kept = fill(4096);
    print_int(array.len(kept));
    print_int(i64(kept[255]));
    print_int(i64(kept[257]));

    const before = gc_live_objects();
    churn(200);
    gc_collect();
    gc_trace();
    const after = gc_live_objects();
    if (after <= before + 4) { print("reclaimed"); }

    // `kept` is still alive and still holds what it did.
    if (kept[4095] != u8(4095)) { print("corrupted"); }
    return 0;
}
