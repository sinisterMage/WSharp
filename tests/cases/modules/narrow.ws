// A service whose state and methods deal in values narrower than a machine
// word. Everything crossing between workers goes through a buffer of whole
// words, so a narrow value writes part of a word and the runtime ships all of
// it -- which is why that buffer is zeroed before anything is written to it.
pub const State = struct { seen: u32, flag: bool, byte: u8 };

pub fn init(start: u32, flag: bool) State {
    return State{ .seen = start, .flag = flag, .byte = 0 };
}

pub fn take(s: State, b: u8, on: bool) u32 {
    s.byte = b;
    s.flag = on;
    s.seen = s.seen + u32(b);
    return s.seen;
}

pub fn byte(s: State) u8 { return s.byte; }

pub fn flag(s: State) bool { return s.flag; }
