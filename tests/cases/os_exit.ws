// expect: before
// exit: 3
// `os.exit` ends the process where it stands.
//
// `main` returning is the ordinary way out and stops the workers on the way;
// this is the one for a program that wants out from somewhere else, or from
// inside a worker, and it stops nothing -- an exit that a worker refusing to
// stop can block is not an exit. Sockets are still released and
// `WSHARP_GC_STATS=1` still reports, so it looks the same from outside.
//
// The line after it never runs, and W# will not tell you so: nothing in the
// type system says "does not return", which is `os.exec`'s position too.
const os = @import("std/os");

fn main() i64 {
    print("before");
    os.exit(3);
    print("after");
    return 0;
}
