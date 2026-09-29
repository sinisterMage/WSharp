// expect: ok 128
// expect: refused 1 this document nests more than 128 deep
// expect: refused 1 this document nests more than 128 deep
// expect: refused 1 this document nests more than 128 deep
//
// #49: `std/toml` had no nesting bound, and a few thousand unterminated `[`
// took most of a minute to refuse -- the fuzzer's timeout called it a hang.
// Arrays and inline tables now nest at most `MAX_DEPTH` deep, like `std/json`'s,
// and a document nested deeper is refused with a message, however deep and
// whether or not it is ever closed.
const toml = @import("std/toml");
const text = @import("std/str");

fn say(doc: toml.Doc) void {
    if (doc.ok) {
        print(text.concat("ok ", text.from_int(toml.MAX_DEPTH)));
    } else {
        print(text.concat(text.concat(text.concat("refused ", text.from_int(doc.line)), " "), doc.message));
    }
    return;
}

fn main() i64 {
    const deepest = text.concat(text.concat("a = ", text.repeat("[", toml.MAX_DEPTH)),
        text.repeat("]", toml.MAX_DEPTH));
    say(toml.parse(deepest));

    const one_more = text.concat(text.concat("a = ", text.repeat("[", toml.MAX_DEPTH + 1)),
        text.repeat("]", toml.MAX_DEPTH + 1));
    say(toml.parse(one_more));

    // The fuzzer's input, near enough: thousands deep and never closed.
    say(toml.parse(text.concat("basic = ", text.repeat("[", 5872))));

    // Inline tables count too, and mixing the two does not reset the count.
    say(toml.parse(text.concat("t = ", text.repeat("{ a = [", 100))));
    return 0;
}
