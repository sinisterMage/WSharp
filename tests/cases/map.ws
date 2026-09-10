// expect: 0
// expect: 3
// expect: 1
// expect: 2
// expect: 3
// expect: -1
// expect: true
// expect: false
// expect: overwritten
// expect: 3
// expect: removed alpha
// expect: 2
// expect: -1
// expect: alpha is back
// expect: 3
// expect: 3
// expect: alpha=9
// expect: gamma=3
// expect: beta=2
// expect: 0
// expect: 0
// `std/map`: a hash table with `str` keys.
//
// `std` went a long time without one, and both places that wanted one said so:
// `std/toml` scans parallel key and value lists and `ingot/pubgrub` keeps a
// list "for `std`'s reason: there is no map". An association list is right for
// a dozen headers and wrong for a prepared-statement cache.
//
// What is checked here is the part open addressing gets wrong: a removal is a
// tombstone rather than an empty slot, because emptying it would end a probe
// run that other keys are still reached through -- so `alpha` removed and put
// back has to be findable, and `beta` has to stay findable throughout.
//
// Iteration order is the table's, which is the hash's. It is asserted here
// because it has to be *something* for a case to check, and because a change
// to `str.hash` should be a decision rather than a surprise.
const map = @import("std/map");
const array = @import("std/array");
const text = @import("std/str");

fn show[V](m: map.Map[V], key: str) void {
    print_int(map.get(m, key) orelse -1);
    return;
}

fn main() i64 {
    var m: map.Map[i64] = map.new();
    print_int(map.len(m));

    map.set(m, "alpha", 1);
    map.set(m, "beta", 2);
    map.set(m, "gamma", 3);
    print_int(map.len(m));
    show(m, "alpha");
    show(m, "beta");
    show(m, "gamma");
    show(m, "delta");

    if (map.has(m, "beta")) { print("true"); } else { print("false"); }
    if (map.has(m, "delta")) { print("true"); } else { print("false"); }

    map.set(m, "alpha", 9);
    if ((map.get(m, "alpha") orelse 0) == 9) { print("overwritten"); }
    print_int(map.len(m));

    if (map.remove(m, "alpha")) { print("removed alpha"); }
    print_int(map.len(m));
    show(m, "alpha");
    // The probe run `alpha` was part of still has to reach the others.
    if (map.has(m, "beta") and map.has(m, "gamma")) { print("alpha is back"); }
    map.set(m, "alpha", 9);
    print_int(map.len(m));

    print_int(array.len(map.keys(m)));
    for (m) |e| {
        print(text.concat(text.concat(e.key, "="), text.from_int(e.value)));
    }

    map.clear(m);
    print_int(map.len(m));
    print_int(array.len(map.keys(m)));
    return 0;
}
