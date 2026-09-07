// The command line, which is the first thing a program written in W# needed
// that W# could not say: `main` takes no arguments, so the arguments arrive
// out of band and are read through `std/os`.
// args: one two --three
// expect: 3
// expect: one
// expect: two
// expect: --three
// expect: <unset>
// expect: yes
// expect: absolute
const array = @import("std/array");
const os = @import("std/os");
const path = @import("std/path");
const text = @import("std/str");

fn ok(p: str) str {
    if (text.len(p) == 0) { return "no"; }
    return "yes";
}

fn main() i64 {
    const args = os.args();
    print_int(array.len(args));
    // A leading `-` is not this driver's flag once the file has been named,
    // and nothing is interpreted on the way through.
    for (args) |a| { print(a); }

    // An environment variable that is not set is null rather than an error:
    // not being set is the ordinary case.
    print(os.get("WSHARP_A_VARIABLE_NOBODY_SETS") orelse "<unset>");
    // And one that every process has. Windows matches a variable's name
    // without regard to case, so one spelling reaches all three platforms.
    print(if (os.get("PATH")) |p| ok(p) else "no");

    // The working directory, which is fallible where `home` and `temp_dir` are
    // not: those ask the environment, and this asks the kernel about a
    // directory that can have been removed since the process entered it.
    //
    // Only that it is absolute is checked, since what it is depends on where
    // the suite was started. Windows answers with `\`, which `path.normalise`
    // is what turns into a `/` -- `std/os` never rewrites one, because a path
    // is arithmetic and the environment is a fact about the process.
    const here = path.normalise(os.cwd() catch return 1);
    print(if (path.is_absolute(here)) "absolute" else "relative");
    return 0;
}
