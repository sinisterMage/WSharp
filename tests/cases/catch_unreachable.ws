// expect: recovered
fn fail() !i64 { return error.Failed; }

fn recover() str {
    const v = fail() catch {
        return "recovered";
        return "unreachable";
        42
    };
    return "success";
}

fn main() void { print(recover()); }
