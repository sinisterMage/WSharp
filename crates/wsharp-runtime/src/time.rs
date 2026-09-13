//! Duration measurement is independent of civil time and clock corrections.
use crate::builtins::{Builtin, BuiltinTy as B, ERROR_BAD_FORMAT};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

#[unsafe(no_mangle)]
pub extern "C" fn ws_time_monotonic_ms() -> i64 {
    unsafe { crate::gc::checkpoint() };
    static START: OnceLock<Instant> = OnceLock::new();
    START
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
#[unsafe(no_mangle)]
pub extern "C" fn ws_time_sleep_ms(ms: i64) -> i64 {
    unsafe { crate::gc::checkpoint() };
    if ms < 0 {
        return ERROR_BAD_FORMAT;
    }
    crate::worker::blocking(|| std::thread::sleep(Duration::from_millis(ms as u64)));
    0
}
pub(crate) fn builtins() -> Vec<Builtin> {
    vec![
        Builtin {
            module: "std/time",
            name: "monotonic_ms",
            params: &[],
            ret: B::I64,
            link: "ws_time_monotonic_ms",
            ptr: ws_time_monotonic_ms as *const u8,
        },
        Builtin {
            module: "std/time",
            name: "sleep_ms",
            params: &[B::I64],
            ret: B::ErrUnion(&B::Void, &["BadFormat"]),
            link: "ws_time_sleep_ms",
            ptr: ws_time_sleep_ms as *const u8,
        },
    ]
}
