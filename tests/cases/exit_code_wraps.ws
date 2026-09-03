// exit: 0
// The process exit status is the low byte of `main`'s result, as for a C
// program: the operating system only has eight bits for it, so 256 wraps to
// 0 and looks like success. Return small values if the status matters.
fn main() i64 { return 256; }
