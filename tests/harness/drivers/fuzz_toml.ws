// A fuzz target for `std/toml`: read a file, parse it, say what came of it.
//
// The sibling of `fuzz_json.ws` and the same shape. `std/toml` deliberately has
// no depth bound where `std/json` has `MAX_DEPTH`, on the argument that a
// manifest is a file with an author -- so a nesting bomb here is an *expected*
// stack overflow rather than a defect, and the harness's dictionary reflects
// that by not building one for this target.
//
//   wsharp run tests/harness/drivers/fuzz_toml.ws <file>
const array = @import("std/array");
const io = @import("std/io");
const os = @import("std/os");
const text = @import("std/str");
const toml = @import("std/toml");

fn main() i64 {
    const args = os.args();
    if (array.len(args) < 1) { print("usage: fuzz_toml <file>"); return 2; }
    const src = io.read_file(args[0]) catch {
        print("unreadable");
        return 2;
    };
    const doc = toml.parse(src);
    if (doc.ok) {
        // Walk the keys as well as accepting the document, for the reason
        // `fuzz_json.ws` asks the value its kind: a table built wrong is as
        // much a defect as a document taken wrongly.
        const keys = toml.keys(doc.root);
        print(text.concat("ok keys=", text.from_int(array.len(keys))));
        return 0;
    }
    print(text.concat("refused ", text.concat(text.from_int(doc.line),
        text.concat(" ", doc.message))));
    return 0;
}
