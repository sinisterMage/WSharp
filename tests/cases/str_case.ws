// expect: CONTENT-LENGTH
// expect: content-length
// expect: ABC123!
// expect: hello-world
// expect: hello world
// expect: aa
// expect: xyx
// expect: aXXa
// `to_upper` beside `to_lower`, and `replace`.
//
// ASCII only, deliberately: a header name is ASCII by definition and case
// folding anything else needs a Unicode table this runtime does not carry.
// `replace` is non-overlapping and left to right, so replacing `"aa"` in
// `"aaa"` finds one occurrence and leaves the third `a` alone; an empty needle
// answers with the subject rather than putting a copy between every byte,
// which is `split`'s rule read the same way.
const text = @import("std/str");

fn main() i64 {
    print(text.to_upper("Content-Length"));
    print(text.to_lower("Content-Length"));
    print(text.to_upper("abc123!"));

    print(text.replace("hello world", " ", "-"));
    print(text.replace("hello world", "", "-"));
    print(text.replace("aaa", "aa", "a"));
    print(text.replace("xyx", "q", "z"));
    // The replacement is not rescanned: one `X` in, two `X` out, and the
    // search resumes past what was written.
    print(text.replace("aXa", "X", "XX"));
    return 0;
}
