// `for` over a list whose elements are structs.
//
// The element type has to reach the loop variable, and the only thing that can
// carry it is `next`'s return type -- which is `?T` for a `T` the call is
// generic in. A dispatched call trials each candidate under a snapshot and
// rolls it back, so the binding that says which `T` this one is has to be made
// again afterwards; without that the loop variable's type is a variable
// nothing ever pins, and the two things a body does with it both go wrong
// quietly: a field read has nothing to look the field up in, and an overloaded
// call sees an argument that overlaps every candidate without a test and
// compiles to a static call to the most specific one.
//
// `for_list.ws` walks a list of `i64` and a list of `str`, and passes without
// any of this, because `print_int` and `str.concat` in the body pin the
// element type from the other end. A struct has nothing that does.
// expect: ada 36
// expect: grace 45
// expect: base
// expect: middle
// expect: leaf
const list = @import("std/list");
const str = @import("std/str");

const Person = struct { name: str, age: i64 };

// Three levels, so "the most specific overload" and "the right overload" are
// different answers for two of the three.
const Base = struct { };
const Middle = struct : Base { };
const Leaf = struct : Middle { };

fn which(v: Base) str { return "base"; }
fn which(v: Middle) str { return "middle"; }
fn which(v: Leaf) str { return "leaf"; }

fn main() i64 {
    var people = list.new();
    list.push(people, Person{ .name = "ada", .age = 36 });
    list.push(people, Person{ .name = "grace", .age = 45 });

    // A field read: needs the loop variable to have a type at all.
    for (people) |p| {
        print(str.concat(p.name, str.concat(" ", str.from_int(p.age))));
    }

    // A dispatched call: needs it to be the *right* type. Each of these is
    // held at its own type, so a call that answered "leaf" three times would
    // be picking the most specific overload rather than dispatching.
    var shapes = list.new();
    const b: Base = Base{ };
    const m: Base = Middle{ };
    const l: Base = Leaf{ };
    list.push(shapes, b);
    list.push(shapes, m);
    list.push(shapes, l);
    for (shapes) |s| { print(which(s)); }
    return 0;
}
