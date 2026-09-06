// A worker that reads. It joins a group by name, which is how two workers that
// have never met agree on a topic and on where they have got to.
const broker = @import("std/broker");
const producer = @import("./producer.ws");

pub const State = struct { c: broker.Consumer[producer.Job] };

pub fn init(topic: str, group: str) State {
    var t: broker.Topic[producer.Job] = broker.topic(topic, 2);
    return State{ .c = broker.subscribe(t, group) };
}

/// Read everything waiting, and say what the numbers add up to. A copy per
/// consumer: the message waits in the log as bytes, belonging to no heap.
pub fn drain(s: State) i64 {
    var total = 0;
    while (broker.next(s.c)) |m| { total += m.n; }
    broker.commit(s.c);
    return total;
}
