// error: a message cannot be `i64`
// error: cannot be sent to another worker
const broker = @import("std/broker");

const Holder = struct { f: fn(i64) i64 };
fn twice(x: i64) i64 { return x * 2; }

fn main() i64 {
    var numbers: broker.Topic[i64] = broker.topic("numbers", 1);
    broker.publish(numbers, "a", 1);

    var bad: broker.Topic[Holder] = broker.topic("bad", 1);
    broker.publish(bad, "a", Holder{ .f = twice });
    return 0;
}
