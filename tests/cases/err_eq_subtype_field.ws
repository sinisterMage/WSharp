// error: cannot be compared with `==`
// error: Sub.f
// A comparison written at a supertype has to be able to compare every subtype,
// because that is what it will be handed: two `Sub`s compared at `Base`
// compare `Sub`'s fields. So a subtype with a field `==` cannot reach makes the
// *supertype* uncomparable, and says which subtype and which field.
//
// Which is the honest answer rather than a limitation: the alternative is a
// comparison at `Base` that silently ignores the half of the value `Base` does
// not declare.
const Base = struct { n: i64 };
const Sub = struct : Base { f: fn(i64) i64 };

fn double(x: i64) i64 { return x * 2; }

fn main() i64 {
    const a: Base = Sub{ .n = 1, .f = double };
    if (a == a) { print("never"); }
    return 0;
}
