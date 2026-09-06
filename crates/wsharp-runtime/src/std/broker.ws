// Named topics, partitioned logs, consumer groups.
//
// RPC is for when the caller needs the answer. This is for when it does not,
// or when more than one worker wants the same message.
//
// The handles are numbers, because a topic belongs to the process rather than
// to any one worker's heap. What makes them typed is here: `Topic[M]` and
// `Consumer[M]` carry the message type, so a topic of `OrderPlaced` cannot be
// published to with an `OrderCancelled` and `poll` produces the type the topic
// was declared with.

/// A topic, and what it carries.
///
/// `sample` is always null. It is what makes `M` a parameter of the struct
/// rather than a name nothing mentions -- and so what makes the compiler check
/// that a publisher and a consumer agree about it.
pub const Topic = struct[M] { id: i64, sample: ?M };

/// One reader's place in a group's progress through a topic.
pub const Consumer = struct[M] { id: i64, sample: ?M };

/// The topic of this name, made if it does not exist yet.
///
/// Named rather than handed out, so that two workers which have never met can
/// agree on one by spelling it the same way. `partitions` is how many logs it
/// is split into: messages sharing a key stay in order with each other, and
/// that is the only ordering anyone can rely on.
pub fn topic[M](name: str, partitions: i64) Topic[M] {
    return Topic{ .id = raw_topic(name, partitions), .sample = null };
}

/// Append `m`, in the partition `key` belongs to.
pub fn publish[M](t: Topic[M], key: str, m: M) void {
    const at = raw_publish(t.id, key, m);
}

/// Join a consumer group, starting where the group has committed.
pub fn subscribe[M](t: Topic[M], group: str) Consumer[M] {
    return Consumer{ .id = raw_subscribe(t.id, group), .sample = null };
}

/// The next message this consumer has not read, or null when it has caught up.
///
/// A copy: the message waits in the log as bytes, belonging to no heap, and
/// each consumer builds its own in its own.
pub fn next[M](c: Consumer[M]) ?M {
    return raw_poll(c.id);
}

/// Say the group has got this far, so a consumer replacing this one starts
/// here. Until then a message that was read but not committed is read again,
/// which is what at-least-once means.
pub fn commit[M](c: Consumer[M]) void {
    raw_commit(c.id);
}

/// Put one partition's position back. Replay is not a feature but the absence
/// of one: the log is still there, and this is where to start reading it.
pub fn seek[M](c: Consumer[M], partition: i64, offset: i64) void {
    raw_seek(c.id, partition, offset);
}

/// How many messages `t` holds, across every partition.
pub fn len[M](t: Topic[M]) i64 {
    return raw_len(t.id);
}
