// Heap objects held across blocking calls, with the heap growing all the while.
//
// Every `read` here parks this worker in a syscall, which is a window in which
// its collector may walk the stack and run a pause -- so the strings the list
// is holding must survive being moved while the thread was not looking. Under
// `--gc-stress` the whole suite runs again collecting at every allocation,
// which is what makes this more than a smoke test.
// expect: 200
// expect: all intact
const net = @import("std/net");
const list = @import("std/list");

fn main() i64 {
    const l = net.listen("127.0.0.1", 0, 8) catch return 1;
    const port = net.local_port(l) catch return 2;
    const client = net.connect("127.0.0.1", port) catch return 3;
    const served = net.accept(l) catch return 4;

    var kept = list.new();
    var i = 0;
    while (i < 200) : (i += 1) {
        net.write_all(client, "chunk") catch return 5;
        // Allocated by a builtin, one per round, and held from here on.
        const got = net.read_exactly(served, 5) catch return 6;
        list.push(kept, got);
    }
    print_int(list.len(kept));

    var intact = true;
    var j = 0;
    while (j < list.len(kept)) : (j += 1) {
        const s = list.get(kept, j);
        if (s != "chunk") { intact = false; }
    }
    if (intact) { print("all intact"); } else { print("corrupted"); }

    net.close(client);
    net.close(served);
    net.close_listener(l);
    return 0;
}
