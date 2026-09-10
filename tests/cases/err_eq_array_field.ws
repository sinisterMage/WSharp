// error: cannot be compared with `==`
// error: Bag.xs
// error: []i64
// `==` on a struct compares its fields, so a field it cannot compare is a
// compile error rather than a comparison that quietly means something else --
// two arrays holding the same numbers are two objects, and answering either
// "equal" or "not equal" for them would be a decision made here rather than by
// the person writing it.
//
// The diagnostic names the field that is at fault and not merely the type the
// comparison was written on: the walk went `Holder` -> `Bag` -> `xs`, and
// `Holder.inner` is not where to look.
const Bag = struct { xs: []i64 };
const Holder = struct { inner: ?Bag };

fn main() i64 {
    const a = Holder{ .inner = null };
    if (a == a) { print("never"); }
    return 0;
}
