// A top-level `const` array is one object shared by every worker in the
// process, and W# has no mutable globals -- the workers' design rests on it,
// since a worker's state has to be an explicit value passed in and out. So
// writing an element of one is reported rather than raced over.
// error: `K` is a top-level `const` array, which cannot be written
// error: shared by every worker
const K = []u32{ 1, 2, 3 };

fn main() i64 {
    K[0] = 9;
    return 0;
}
