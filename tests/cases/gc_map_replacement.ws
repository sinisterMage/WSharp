// expect: 12
// timeout: 1200
// Replacing a map in a retained struct used to leave freed fresh objects in
// the nursery. Normal GC crashed when the marker followed those stale roots;
// collecting at every allocation hid the failure. Keep the original workload.
//
// `timeout: 1200` because the workload is 1.2 M allocations and `--gc-stress`
// collects at every one of them. On the slowest release triples that is ~940 s
// -- under the 1200 s bound, over `parity.sh`'s 300 s default. Across two
// nights it crossed the default on a different set of x86-64 runners each time
// while aarch64, which does it in ~256 s, agreed on all three modes; the
// whole-corpus `--gc-stress` pass in the Rust suite (which has no per-case
// bound) finishes it, so this is cost and not a hang (#50). The bound is named
// here rather than raised globally so a real hang on any other case still
// surfaces in 300 s.
const map = @import("std/map");
const text = @import("std/str");
const Entry = struct { size: i64, modified: i64, digest: str };
const State = struct { entries: map.Map[Entry] };

fn main() i64 {
    const m: map.Map[Entry] = map.new();
    const state = State{ .entries = m };
    var i = 0;
    while (i < 100000) : (i += 1) {
        const fresh: map.Map[Entry] = map.new();
        var j = 0;
        while (j < 12) : (j += 1) {
            const name = text.concat("file", text.from_int(j));
            map.set(fresh, name, Entry{ .size = 1000, .modified = 0, .digest = name });
        }
        state.entries = fresh;
    }

    // Check the graph behind the retained map, not just its scalar count.
    var j = 0;
    while (j < 12) : (j += 1) {
        const name = text.concat("file", text.from_int(j));
        if (map.get(state.entries, name)) |entry| {
            if (entry.size != 1000 or entry.modified != 0 or !text.eq(entry.digest, name)) {
                return 1;
            }
        } else { return 2; }
    }
    print(map.len(state.entries));
    return 0;
}
