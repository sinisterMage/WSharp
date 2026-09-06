// error: `helper` is private to the module that declares it
// error: `STEP` is private to the module that declares it
const vis = @import("./modules/vis.ws");

fn main() i64 {
    print_int(vis.helper());
    return vis.STEP;
}
