// expect: hello from the worker
// expect: a request
// expect: 1
// A worker whose `init` blocks, which is the shape `@spawn` exists for.
//
// `init` runs on the worker thread and the message loop is entered only after
// it returns, so an acceptor is written *as* an `init`. That means the very
// first thing a real worker does is a blocking syscall, and a blocking call
// runs inside a safe region -- which a thread may not enter while the runtime
// holds pinned roots on it, because those live on the thread and a collector
// offered a parked worker holding them declines and waits instead.
//
// The arguments `@spawn` copied into this heap were pinned for the whole of
// `init`, so every acceptor did exactly that. They come off before generated
// code is entered now: `unpack` returning is the last moment anything can
// allocate, and the callee's prologue roots its own parameters before its first
// safepoint, so there is no safepoint in between.
//
// Single-threaded on the socket side, as every networking case here is: the
// connection is made before the worker exists and completes into the backlog,
// so `accept` has something waiting and nothing can deadlock.
const net = @import("std/net");
const daemon = @import("./modules/daemon.ws");

fn main() i64 {
    const l = net.listen("127.0.0.1", 0, 8) catch return 1;
    const port = net.local_port(l) catch return 2;
    const c = net.connect("127.0.0.1", port) catch return 3;
    net.write_all(c, "a request") catch return 4;
    // The write half only: the worker reads to the end of the stream and the
    // connection stays open the other way, which is what `close` cannot say.
    net.shutdown(c, false, true) catch return 5;

    const w = @spawn(daemon, l, "hello from the worker") catch return 6;
    print_int(w.served() catch -1);
    @join(w) catch return 7;
    net.close(c);
    net.close_listener(l);
    return 0;
}
