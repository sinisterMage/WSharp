// expect: 2000
// expect: 2000 found
// expect: 1000
// expect: 1000 found after removing every other one
// expect: 2000
// expect: 2000 found after putting them back
// expect: 0 strangers found
// expect: 32
// expect: 32
// A map that grows through several resizes, and one that fills with tombstones.
//
// Two thousand keys is enough to resize eight times from the starting eight
// slots, so the re-probe in `resize` is exercised rather than assumed -- it
// carries the stored hash across rather than hashing every key again, and a
// mistake there loses entries silently.
//
// Removing every other key and putting them all back is the other half: a
// tombstone costs a probe exactly as a live entry does, so the load factor is
// measured against live-plus-dead. Measured against the live count alone, this
// loop would fill the table with tombstones, never grow, and end up probing the
// whole table for every lookup -- which is a test that passes slowly rather
// than one that fails, so the count is what is checked.
//
// `with_capacity` asks for room for `n` and gets it: the table grows at seven
// eighths, so the array behind it is twice `n` rounded up to a power of two.
const map = @import("std/map");
const text = @import("std/str");
const array = @import("std/array");

fn key(i: i64) str { return text.concat("k", text.from_int(i)); }

fn main() i64 {
    var m: map.Map[i64] = map.new();
    var i = 0;
    while (i < 2000) : (i += 1) { map.set(m, key(i), i); }
    print_int(map.len(m));

    var found = 0;
    i = 0;
    while (i < 2000) : (i += 1) {
        if ((map.get(m, key(i)) orelse -1) == i) { found += 1; }
    }
    print(text.concat(text.from_int(found), " found"));

    i = 0;
    while (i < 2000) : (i += 2) {
        if (!map.remove(m, key(i))) { return 1; }
    }
    print_int(map.len(m));

    found = 0;
    i = 1;
    while (i < 2000) : (i += 2) {
        if ((map.get(m, key(i)) orelse -1) == i) { found += 1; }
    }
    print(text.concat(text.from_int(found), " found after removing every other one"));

    i = 0;
    while (i < 2000) : (i += 2) { map.set(m, key(i), i); }
    print_int(map.len(m));

    found = 0;
    i = 0;
    while (i < 2000) : (i += 1) {
        if ((map.get(m, key(i)) orelse -1) == i) { found += 1; }
    }
    print(text.concat(text.from_int(found), " found after putting them back"));

    // Nothing that was never put in is ever found, however full the table is.
    var strangers = 0;
    i = 0;
    while (i < 500) : (i += 1) {
        if (map.has(m, text.concat("absent", text.from_int(i)))) { strangers += 1; }
    }
    print(text.concat(text.from_int(strangers), " strangers found"));

    // Room asked for is room given: 32 entries go in and none of them makes
    // the table resize, which is what `with_capacity`'s doubling is for.
    var room: map.Map[str] = map.with_capacity(32);
    i = 0;
    while (i < 32) : (i += 1) { map.set(room, key(i), key(i)); }
    print_int(map.len(room));
    print_int(array.len(map.keys(room)));
    return 0;
}
