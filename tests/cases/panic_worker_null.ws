// panic: read through a null reference
//
// The same read through null as `panic_null_field.ws`, on a worker's thread.
const faulty = @import("./modules/faulty.ws");

fn main() i64 {
    const w = @spawn(faulty, 3) catch return 1;
    print(w.first_x() catch -1);
    print("unreachable");
    return 0;
}
