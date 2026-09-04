// A second fixture, to show a module importing another one.
const geometry = @import("./geometry.ws");

const Box = struct[T] { value: T };

fn origin() geometry.Point { return geometry.Point{ .x = 0, .y = 0 }; }
fn wrap[T](v: T) Box[T] { return Box{ .value = v }; }
