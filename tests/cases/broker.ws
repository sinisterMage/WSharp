// The message broker: named topics, partitioned logs, consumer groups.
//
// RPC is for when the caller needs the answer. This is for when it does not,
// or when more than one worker wants the same message. The shape is Kafka's,
// because that shape is what makes two useful things possible at once: a
// consumer that has fallen behind can catch up, and a consumer that has died
// can be replaced by one that starts where its group had got to.
//
// The part worth noticing is what the broker does *not* do. A subscriber set
// is an overload set, and choosing between its members is the dispatcher --
// one subtract and one unsigned compare on the type id in the message's own
// header. That the same pattern turns up here and in the status lattice, in
// unrelated features, is the argument that it was the right one.
// expect: 2
// expect: placed 7
// expect: cancelled sold out
// expect: 3
// expect: placed 7
// expect: cancelled sold out
// expect: 3
// expect: 0
// expect: placed 7
// expect: cancelled sold out
// expect: 3
// expect: placed 7
// expect: cancelled sold out
// expect: 3
const broker = @import("std/broker");
const str = @import("std/str");

const Event = struct { at: i64 };
const OrderPlaced = struct : Event { id: i64 };
const OrderCancelled = struct : Event { reason: str };

// The subscriber set. Nothing in the broker knows these exist: `next` hands
// back the topic's message type, and the call below picks by the runtime type
// id the copy carried with it.
fn handle(e: Event) i64 { print("something else"); return 0; }
fn handle(e: OrderPlaced) i64 { print(str.concat("placed ", str.from_int(e.id))); return 1; }
fn handle(e: OrderCancelled) i64 { print(str.concat("cancelled ", e.reason)); return 2; }

fn drain(c: broker.Consumer[Event]) i64 {
    var seen = 0;
    while (broker.next(c)) |m| { seen += handle(m); }
    return seen;
}

fn main() i64 {
    var orders: broker.Topic[Event] = broker.topic("orders", 2);
    broker.publish(orders, "a", OrderPlaced{ .at = 1, .id = 7 });
    broker.publish(orders, "b", OrderCancelled{ .at = 2, .reason = "sold out" });
    print_int(broker.len(orders));

    // Two groups on one topic: each gets every message, because a group has
    // its own offsets.
    var billing: broker.Consumer[Event] = broker.subscribe(orders, "billing");
    print_int(drain(billing));
    broker.commit(billing);

    var audit: broker.Consumer[Event] = broker.subscribe(orders, "audit");
    print_int(drain(audit));
    // Not committed, deliberately.

    // Caught up: nothing left for this consumer.
    print_int(drain(billing));

    // Replay is not a feature but the absence of one -- the log is still
    // there, and this is where to start reading it again.
    broker.seek(billing, 0, 0);
    broker.seek(billing, 1, 0);
    print_int(drain(billing));

    // A new consumer in an uncommitted group reads what its predecessor read
    // but never committed: that is what at-least-once means.
    var audit2: broker.Consumer[Event] = broker.subscribe(orders, "audit");
    print_int(drain(audit2));
    return 0;
}
