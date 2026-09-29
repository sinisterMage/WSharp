// panic: stack overflow
//
// A runaway recursion on a worker's thread, whose stack is a different size
// from the main thread's and was set up by a different party.
const faulty = @import("./modules/faulty.ws");

fn main() i64 {
    const w = @spawn(faulty, 3) catch return 1;
    print(w.recurse() catch -1);
    print("unreachable");
    return 0;
}
