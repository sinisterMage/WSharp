//! Signal handlers only set an atomic bit. W# polls on an ordinary worker stack;
//! it is never called asynchronously, and no allocator or lock runs in a handler.
use crate::builtins::{Builtin, BuiltinTy as B, ERROR_IO_FAILED};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

static PENDING: AtomicU32 = AtomicU32::new(0);
static INSTALLED: Mutex<Option<platform::Previous>> = Mutex::new(None);

#[cfg(unix)]
mod platform {
    use super::*;
    unsafe extern "C" {
        fn signal(number: i32, handler: usize) -> usize;
    }
    pub struct Previous {
        interrupt: usize,
        terminate: usize,
    }
    extern "C" fn received(number: i32) {
        PENDING.fetch_or(if number == 2 { 1 } else { 2 }, Ordering::Relaxed);
    }
    pub fn install() -> Result<Previous, i64> {
        let interrupt = unsafe { signal(2, received as *const () as usize) };
        if interrupt == usize::MAX {
            return Err(ERROR_IO_FAILED);
        }
        let terminate = unsafe { signal(15, received as *const () as usize) };
        if terminate == usize::MAX {
            unsafe { signal(2, interrupt) };
            return Err(ERROR_IO_FAILED);
        }
        Ok(Previous {
            interrupt,
            terminate,
        })
    }
    pub fn restore(previous: Previous) {
        unsafe {
            signal(2, previous.interrupt);
            signal(15, previous.terminate);
        }
    }
}
#[cfg(windows)]
mod platform {
    use super::*;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetConsoleCtrlHandler(handler: unsafe extern "system" fn(u32) -> i32, add: i32) -> i32;
    }
    pub struct Previous;
    unsafe extern "system" fn received(event: u32) -> i32 {
        if event == 0 || event == 1 {
            PENDING.fetch_or(1, Ordering::Relaxed);
            return 1;
        }
        0 // Window close/logoff/shutdown have separate OS deadlines; not claimed.
    }
    pub fn install() -> Result<Previous, i64> {
        if unsafe { SetConsoleCtrlHandler(received, 1) } == 0 {
            return Err(ERROR_IO_FAILED);
        }
        Ok(Previous)
    }
    pub fn restore(_: Previous) {
        unsafe { SetConsoleCtrlHandler(received, 0) };
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ws_os_catch_signals() -> i64 {
    unsafe { crate::gc::checkpoint() };
    crate::worker::blocking(|| {
        let mut installed = INSTALLED.lock().unwrap_or_else(|e| e.into_inner());
        if installed.is_some() {
            return 0;
        }
        match platform::install() {
            Ok(previous) => {
                *installed = Some(previous);
                0
            }
            Err(e) => e,
        }
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn ws_os_take_signal() -> i64 {
    unsafe { crate::gc::checkpoint() };
    let old = PENDING
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |bits| {
            Some(if bits & 1 != 0 { bits & !1 } else { bits & !2 })
        })
        .unwrap_or(0);
    if old & 1 != 0 {
        2
    } else if old & 2 != 0 {
        15
    } else {
        0
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn ws_os_restore_signals() {
    unsafe { crate::gc::checkpoint() };
    restore();
}
pub fn restore() {
    crate::worker::blocking(|| {
        if let Some(previous) = INSTALLED.lock().unwrap_or_else(|e| e.into_inner()).take() {
            platform::restore(previous);
        }
        PENDING.store(0, Ordering::Relaxed);
    });
}
pub(crate) fn builtins() -> Vec<Builtin> {
    vec![
        Builtin {
            module: "std/os",
            name: "raw_catch_signals",
            params: &[],
            ret: B::ErrUnion(&B::Void, &["IoFailed"]),
            link: "ws_os_catch_signals",
            ptr: ws_os_catch_signals as *const u8,
        },
        Builtin {
            module: "std/os",
            name: "raw_take_signal",
            params: &[],
            ret: B::I64,
            link: "ws_os_take_signal",
            ptr: ws_os_take_signal as *const u8,
        },
        Builtin {
            module: "std/os",
            name: "raw_restore_signals",
            params: &[],
            ret: B::Void,
            link: "ws_os_restore_signals",
            ptr: ws_os_restore_signals as *const u8,
        },
    ]
}
