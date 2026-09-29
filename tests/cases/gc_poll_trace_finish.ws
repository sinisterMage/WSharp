// Direct trace safepoints and loop polls must coexist without losing a pause.
// The deterministic publication interleavings are in runtime_poll_publication.rs.
// expect: 42
const Node = struct { value: i64, next: ?Node };
fn main() i64 {
    const root = Node{ .value = 42, .next = null };
    var i = 0;
    while (i < 8) : (i += 1) {
        gc_trace_start();
        gc_trace_finish();
    }
    print_int(root.value);
    return 0;
}
