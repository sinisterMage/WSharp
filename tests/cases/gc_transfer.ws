// The deep copy a message makes.
//
// No object is reachable from two workers and nothing is sent by pointer, so
// sending a value means copying the graph reachable from it. `gc_transfer` is
// both halves of that on one worker -- encode out of this heap, decode back
// into it -- which is the whole of the copy except for which heap the second
// half runs on.
//
// The walk goes through `types::for_each_ptr_offset`, the one definition of
// where an object's references are, so an array's elements travel exactly as a
// struct's fields do. Decoding is the interesting half: it builds a graph in
// Rust locals that no stack map describes, so every object it makes is pinned
// on the runtime root list until the graph is finished -- a fourth place a
// heap pointer can live, and one `--gc-stress` checks along with the rest.
// expect: a
// expect: bb
// expect: ccc
// expect: 3
// expect: 1
// expect: 2
// expect: 1
// expect: 7
// expect: 12
// expect: true
// expect: true
// expect: y
// expect: 9
// expect: true
const array = @import("std/array");
const str = @import("std/str");

const Node = struct { value: i64, next: ?Node };
const Pair = struct { name: str, counts: []i64 };

fn main() i64 {
    // Strings: a variable-sized object whose length is in its header.
    const words = gc_transfer([]str{ "a", "bb", "ccc" });
    for (words) |w| { print(w); }
    print_int(array.len(words));

    // A cycle stays a cycle rather than becoming an infinite walk, and is
    // copied once rather than twice.
    var head = Node{ .value = 1, .next = null };
    head.next = Node{ .value = 2, .next = head };
    const nodes = gc_transfer([]Node{ head });
    print_int(nodes[0].value);
    print_int(nodes[0].next.?.value);
    print_int(nodes[0].next.?.next.?.value);

    // A struct holding a string and an array of scalars: two shapes of
    // reference in one object.
    const pairs = gc_transfer([]Pair{ Pair{ .name = "seven", .counts = []i64{ 3, 4 } } });
    print_int(pairs[0].counts[0] + pairs[0].counts[1]);

    // Sharing is preserved: one object referenced twice stays one object.
    var shared = Node{ .value = 5, .next = null };
    const both = gc_transfer([]Node{ shared, shared });
    var first = both[0];
    first.value = 12;
    print_int(both[1].value);

    // And the copy really is a copy: writing to it leaves the original alone.
    print_bool(shared.value == 5);
    print_bool(str.eq(pairs[0].name, "seven"));

    // A copy made *while a trace is marking*. The objects `decode` allocates
    // are born marked, and the half-built graph it is holding is on the
    // runtime root list -- which the pause walks along with the stack, and
    // which the evacuation fix-up repoints along with everything else.
    gc_trace_start();
    const during = gc_transfer([]str{ "x", "y" });
    var later = gc_transfer([]Node{ Node{ .value = 9, .next = null } });
    gc_trace_finish();
    print(during[1]);
    print_int(later[0].value);
    print_bool(gc_traces() > 0);
    return 0;
}
