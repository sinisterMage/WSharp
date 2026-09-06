// A producer worker and two consumer workers, on one topic.
//
// Nothing here is sent by pointer: the producer's `Job` objects are its own,
// they wait in the log as bytes belonging to no heap, and each consumer builds
// its own copy in its own. Two groups means both consumers see everything;
// one group would have meant they shared the work.
// expect: 5
// expect: 10
// expect: 10
// expect: 0
const producer = @import("./modules/producer.ws");
const consumer = @import("./modules/consumer.ws");

fn main() i64 {
    // Subscribed before anything is published, so that the groups start at
    // zero rather than wherever the log had got to.
    const a = @spawn(consumer, "jobs", "billing") catch return 1;
    const b = @spawn(consumer, "jobs", "audit") catch return 1;
    const p = @spawn(producer, "jobs") catch return 1;

    print_int(p.produce(5) catch -1);
    print_int(a.drain() catch -1);
    print_int(b.drain() catch -1);
    // Committed, so there is nothing left for this one.
    print_int(a.drain() catch -1);

    @join(p) catch return 2;
    @join(a) catch return 2;
    @join(b) catch return 2;
    return 0;
}
