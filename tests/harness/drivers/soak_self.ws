// A long-running allocator, so the soak harness can be tested before there is
// anything real to soak.
//
// The soak harness's job is to notice a monotone trend in a process's resident
// size, its descriptor count and its throughput over days. A harness that has
// never been pointed at a process has not been tested, and the first real
// driver is not the thing to discover a broken sampler on -- so this exists to
// be soaked: it allocates continuously, keeps a bounded live set, and prints a
// line every so often so throughput is measurable from outside.
//
// The live set is bounded on purpose. A program whose RSS grows because it is
// *keeping* everything tells the harness nothing about whether the collector
// leaks; this one drops what it makes, so RSS that climbs anyway is the
// collector's and the harness should say so.
//
//   wsharp run tests/harness/drivers/soak_self.ws <batches> <per-batch>
const array = @import("std/array");
const list = @import("std/list");
const os = @import("std/os");
const text = @import("std/str");

const Node = struct { n: i64, label: str, next: ?Node };

/// Build a chain, then drop it. Returns something derived from it so nothing
/// can be optimised away on the strength of the result being unused.
fn churn(depth: i64) i64 {
    var head: ?Node = null;
    var i = 0;
    while (i < depth) {
        head = Node{ .n = i, .label = text.from_int(i), .next = head };
        i = i + 1;
    }
    var total = 0;
    var walk = head;
    while (walk) |node| {
        total = total + node.n + text.len(node.label);
        walk = node.next;
    }
    return total;
}

fn main() i64 {
    const args = os.args();
    var batches = 1000000;
    var depth = 200;
    if (array.len(args) > 0) { batches = text.parse_int(args[0]) catch 1000000; }
    if (array.len(args) > 1) { depth = text.parse_int(args[1]) catch 200; }

    var b = 0;
    var checksum = 0;
    while (b < batches) {
        // A list that is grown and thrown away each batch, beside the chain, so
        // both a container's backing array and a struct graph are exercised.
        const held: list.List[str] = list.new();
        var i = 0;
        while (i < 64) {
            list.push(held, text.concat("item-", text.from_int(i)));
            i = i + 1;
        }
        checksum = checksum + churn(depth) + list.len(held);
        b = b + 1;
        // A heartbeat line every 100 batches: the soak harness counts these to
        // get throughput, and a process that stops printing has stalled even if
        // it is still alive.
        if (b % 100 == 0) {
            print(text.concat("batch ", text.concat(text.from_int(b),
                text.concat(" checksum ", text.from_int(checksum)))));
        }
    }
    print(text.concat("done ", text.from_int(checksum)));
    return 0;
}
