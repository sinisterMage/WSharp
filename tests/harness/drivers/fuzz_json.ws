// A fuzz target for `std/json`: read a file, parse it, say what came of it.
//
// Not a case, which is why it lives here rather than in `tests/cases` -- it
// takes an argument and has no fixed expectation. What is fuzzed is the *input
// file*, so the mutator never has to produce a valid W# program to reach the
// parser, and `std/json.parse` is the whole of what is under test.
//
// Every outcome this program can reach is printed and exits 0. That is the
// point: the harness is looking for the outcomes this program *cannot* print,
// which are a runtime panic, a signal, or not terminating.
//
//   wsharp run tests/harness/drivers/fuzz_json.ws <file>
const array = @import("std/array");
const io = @import("std/io");
const json = @import("std/json");
const os = @import("std/os");
const text = @import("std/str");

fn main() i64 {
    const args = os.args();
    if (array.len(args) < 1) { print("usage: fuzz_json <file>"); return 2; }
    const src = io.read_file(args[0]) catch {
        print("unreadable");
        return 2;
    };
    const doc = json.parse(src);
    if (doc.ok) {
        // Ask the value about itself, so a document that parses is also walked
        // rather than merely accepted: a reader can be wrong about a value it
        // built as easily as about one it refused.
        print(text.concat("ok kind=", text.from_int(json.kind(doc.root))));
        return 0;
    }
    print(text.concat("refused ", text.concat(text.from_int(doc.line),
        text.concat(" ", text.concat(text.from_int(doc.at),
            text.concat(" ", doc.message))))));
    return 0;
}
