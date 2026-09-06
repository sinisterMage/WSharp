// A second fixture, to show a module importing another one.
const geometry = @import("./geometry.ws");

pub const Box = struct[T] { value: T };

pub fn origin() geometry.Point { return geometry.Point{ .x = 0, .y = 0 }; }
pub fn wrap[T](v: T) Box[T] { return Box{ .value = v }; }
