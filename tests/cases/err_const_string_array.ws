// #47: static arrays cannot contain references, even string literals.
// error: a top-level `const` array may not hold `str`
// error: err_const_string_array.ws:5:18
// error: help: it lives in the data section, which the collector never traces
const suffixes = []str{ ".json" };
fn main() void {}
