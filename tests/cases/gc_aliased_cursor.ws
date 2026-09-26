// expect: 250
// expect: 250
// expect: 250
// expect: 250
// A recursive descent reader holds one cursor in every frame at once, and the
// collector has to move it for all of them.
//
// This is the reduced form of the reader in WSharp #31, which returned a
// different number of children on nearly every run -- 34, 56, 128, 167, 255,
// 261 -- for an input with exactly 250 of them. `c` is a single `Cursor`
// object, and `read` recurses with it, so it sits in `main`'s local and in
// each live frame's parameter slot simultaneously. `c.index += 1` writes
// through whichever slot the frame doing the reading holds.
//
// The pause that finishes marking moves everything the roots name, so that the
// concurrent copying which follows cannot move something the program holds.
// It reached the same object once per slot: the first one forwarded it, and
// every later one found an address where the header was, could not read a type
// id out of it, and was handed back the address it came in with -- still
// pointing into a block about to be released. An outer frame's `c.index += 1`
// then went to the abandoned copy and was lost, so the reader either stopped
// early or re-read bytes it had already consumed.
//
// Four rounds rather than the issue's ten, because the suite runs every case
// three times and once of those is under `--gc-stress`.
const array = @import("std/array");
const text = @import("std/str");

const Tree = struct { value: str, children: []Tree };
const Cursor = struct { input: str, index: i64 };

fn character(c: Cursor) str {
    return text.substr(c.input, c.index, c.index + 1);
}

fn read(c: Cursor, len: i64) Tree {
    var t = Tree{ .value = "", .children = []Tree{} };
    if (text.eq(character(c), "(")) {
        c.index += 1;
        while (!text.eq(character(c), ")")) {
            if (c.index >= len) {
                print("premature end");
                return t;
            }
            if (text.eq(character(c), " ")) {
                c.index += 1;
                continue;
            }
            t.children = array.push(t.children, read(c, len));
        }
        c.index += 1;
        return t;
    }
    while (c.index < len and !text.eq(character(c), " ") and !text.eq(character(c), ")")) {
        t.value = text.concat(t.value, character(c));
        c.index += 1;
    }
    return t;
}

fn main() i64 {
    const input = text.concat("(", text.concat(text.repeat("(entry abcdefghijklmnopqrstuvwxyz (field value i64)) ", 250), ")"));
    // Hoisted, because reading it inside the loop condition would be a stack
    // walk per character under `--gc-stress`.
    const len = text.len(input);
    var round = 0;
    while (round < 4) : (round += 1) {
        const c = Cursor{ .input = input, .index = 0 };
        const result = read(c, len);
        print(array.len(result.children));
    }
    return 0;
}
