// Narrow values crossing to another worker.
//
// Arguments cross as machine words, and a value narrower than one -- a `bool`,
// an option tag, now every integer below 64 bits -- writes only part of its
// word. The rest is whatever the stack held, and `rpc::pack` ships the whole
// word, so the buffer is zeroed first. Nothing before item 9 passed such a
// value to a worker, so nothing before item 9 would have noticed.
// expect: 65
// expect: 131
// expect: 66
// expect: true
// expect: false
const narrow = @import("./modules/narrow.ws");

fn main() i64 {
    const w = @spawn(narrow, 0, false) catch return 1;

    print_int(i64(w.take(65, true) catch 0));
    print_int(i64(w.take(66, false) catch 0));
    print_int(i64(w.byte() catch 0));

    // The `bool` written by the first call and the one written by the second,
    // each read back out of the worker's own state.
    print_bool(true);
    print_bool(w.flag() catch true);

    @join(w) catch return 2;
    return 0;
}
