// expect: 3 seeded map traces passed
// Fixed LCG seeds, 128 operations per trace, 24 keys. The oracle is an indexed
// array, independent of the map's hashing/probing. Empty keys, replacement,
// missing removal, clearing, iteration and growth all carry heap references
// through generics and optionals, including the suite's GC-stress pass.
const map = @import("std/map");
const array = @import("std/array");
const text = @import("std/str");
const Record = struct { id: i64, value: str };

fn key(id: i64) str {
    if (id == 0) { return ""; }
    return text.concat("key-", text.from_int(id));
}

fn agrees(m: map.Map[Record], present: []bool, values: []str) bool {
    var count = 0;
    var id = 0;
    while (id < 24) : (id += 1) {
        if (map.has(m, key(id)) != present[id]) { return false; }
        if (map.get(m, key(id))) |r| {
            if (!present[id] or r.id != id or r.value != values[id]) { return false; }
            count += 1;
        } else {
            if (present[id]) { return false; }
        }
    }
    if (map.len(m) != count or array.len(map.keys(m)) != count) { return false; }
    var seen: []bool = array.repeat(24, false);
    var walked = 0;
    for (m) |entry| {
        const r = entry.value;
        if (r.id < 0 or r.id >= 24) { return false; }
        if (seen[r.id] or !present[r.id]) { return false; }
        if (entry.key != key(r.id) or r.value != values[r.id]) { return false; }
        seen[r.id] = true;
        walked += 1;
    }
    return walked == count;
}

fn trace(seed: u32) bool {
    var m: map.Map[Record] = map.new();
    var present: []bool = array.repeat(24, false);
    var values: []str = array.repeat(24, "");
    if (!agrees(m, present, values)) { return false; }
    // Force growth before removals create tombstones.
    var id = 0;
    while (id < 24) : (id += 1) {
        values[id] = text.from_int(id);
        present[id] = true;
        map.set(m, key(id), Record{ .id = id, .value = values[id] });
    }
    var state = seed;
    var step = 0;
    while (step < 128) : (step += 1) {
        state = state * 1664525 + 1013904223;
        id = i64((state >> 16) % 24);
        const op = (state >> 24) % 4;
        if (op < 2) {
            values[id] = text.concat("value-", text.from_int(step));
            present[id] = true;
            map.set(m, key(id), Record{ .id = id, .value = values[id] });
        } else if (op == 2) {
            if (map.remove(m, key(id)) != present[id]) { return false; }
            present[id] = false;
        } else if (step % 31 == 0) {
            map.clear(m);
            present = array.repeat(24, false);
        }
        if (!agrees(m, present, values)) {
            print(seed);
            print(step);
            return false;
        }
    }
    map.clear(m);
    present = array.repeat(24, false);
    return agrees(m, present, values) and !map.remove(m, "");
}

fn main() i64 {
    if (!trace(1) or !trace(0x57534841) or !trace(0xdeadbeef)) { return 1; }
    print("3 seeded map traces passed");
    return 0;
}
