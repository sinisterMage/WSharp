// A heap space is reserved when a worker's heap starts, not touched.
//
// Every worker's heap takes a 64 MiB space on its first allocation, and the
// space was asked of the allocator zeroed *and* aligned to a block -- which the
// system allocator answers with an aligned allocation and a memset of all of
// it. So each worker cost 64 MiB of resident memory before it had allocated 64
// bytes: eight acceptors were half a gigabyte, and a leak smaller than a space
// could not show in the resident set, which is what the Raython soak's gate
// reads. Found by that soak's rehearsal.
//
// Eight workers each allocate a little; the process may grow by less than half
// a space for each. What it measures is Linux's `/proc/self/status`; elsewhere
// there is no such file and the case says its line without asking.
// expect: resident grows by less than half a space per worker
const heaps = @import("./modules/heaps.ws");
const io = @import("std/io");
const text = @import("std/str");

const WORKERS = 8;
const HALF_A_SPACE_KIB = 32768;

/// VmRSS in KiB, or -1 where there is no `/proc/self/status` to read it from.
fn resident_kib() i64 {
    const status = io.read_file("/proc/self/status") catch return -1;
    const at = text.find(status, "VmRSS:");
    if (at < 0) { return -1; }
    const n = text.len(status);
    var i = at + 6;
    var kib = 0;
    while (i < n) : (i += 1) {
        const b = text.byte_at(status, i);
        if (b >= 48 and b <= 57) { kib = kib * 10 + (b - 48); }
        else if (kib > 0) { return kib; }
    }
    return kib;
}

/// Spawn `n` workers, each allocating on its own heap, and answer how much the
/// process grew with all of them alive. Recursive so that every handle stays
/// in a frame of its own until the measurement is taken.
fn grown(n: i64, before: i64) i64 {
    if (n == 0) { return resident_kib() - before; }
    const w = @spawn(heaps) catch return -1;
    const live = w.churn(16) catch return -1;
    const grew = grown(n - 1, before);
    @join(w) catch return -1;
    return grew;
}

fn main() i64 {
    const before = resident_kib();
    const grew = grown(WORKERS, before);
    if (before < 0 or (grew >= 0 and grew < WORKERS * HALF_A_SPACE_KIB)) {
        print("resident grows by less than half a space per worker");
    } else {
        print(text.concat("resident grew by KiB: ", text.from_int(grew)));
    }
    return 0;
}
