// A worker that publishes. Its messages are copies by the time anyone reads
// them, so nothing it allocates is ever reachable from a consumer's heap.
const broker = @import("std/broker");
const str = @import("std/str");

pub const Job = struct { n: i64, tag: str };
pub const State = struct { topic: broker.Topic[Job] };

pub fn init(name: str) State {
    return State{ .topic = broker.topic(name, 2) };
}

pub fn produce(s: State, count: i64) i64 {
    var i = 0;
    while (i < count) : (i += 1) {
        broker.publish(s.topic, str.from_int(i), Job{ .n = i, .tag = "job" });
    }
    return broker.len(s.topic);
}
