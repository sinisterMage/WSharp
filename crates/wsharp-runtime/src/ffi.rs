//! Native resources live outside the moving heap. Generated C thunks marshal
//! only scalars, and execute inside worker::blocking while their W# caller's
//! stack remains frozen. C must never re-enter W# or unwind through that call.

use crate::builtins::{
    Builtin, BuiltinTy as B, ERROR_BAD_FORMAT, ERROR_IO_FAILED, ERROR_NOT_FOUND, INLINE,
};
use crate::io::{FallibleI64, FallibleStr};
use crate::strings::{alloc_str, str_bytes};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

mod loader;

struct Library(usize);
impl Drop for Library {
    fn drop(&mut self) {
        unsafe { loader::close(self.0) };
    }
}

#[derive(Default)]
struct Resources {
    next: i64,
    libraries: HashMap<i64, Arc<Library>>,
    symbols: HashMap<i64, (i64, usize)>,
    buffers: HashMap<i64, Box<[u64]>>,
    sizes: HashMap<i64, usize>,
}

impl Resources {
    // Never reuse an id: an old binding must never name a newly opened library.
    fn id(&mut self) -> i64 {
        self.next += 1;
        self.next
    }
}

fn resources() -> &'static Mutex<Resources> {
    static TABLE: OnceLock<Mutex<Resources>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(Resources::default()))
}

thread_local! { static LAST_ERROR: RefCell<String> = const { RefCell::new(String::new()) }; }
fn failure(message: impl Into<String>, tag: i64) -> FallibleI64 {
    LAST_ERROR.with(|last| *last.borrow_mut() = message.into());
    FallibleI64::err(tag)
}

#[unsafe(no_mangle)]
pub extern "C" fn ws_ffi_last_error() -> *mut u8 {
    unsafe { crate::gc::checkpoint() };
    let message = LAST_ERROR.with(|last| last.borrow().clone());
    alloc_str(message.as_bytes())
}

/// # Safety
/// `out` is return storage and `path` is a W# string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_ffi_open(out: *mut FallibleI64, path: *const u8) {
    unsafe { crate::gc::checkpoint() };
    let path = unsafe { str_bytes(path) }.to_vec();
    let result = if path.is_empty() || path.contains(&0) {
        failure(
            "library path must be nonempty and contain no NUL",
            ERROR_BAD_FORMAT,
        )
    } else {
        crate::worker::blocking(|| match loader::open(&path) {
            Ok(handle) => {
                let mut table = resources().lock().unwrap_or_else(|e| e.into_inner());
                let id = table.id();
                table.libraries.insert(id, Arc::new(Library(handle)));
                FallibleI64::ok(id)
            }
            Err(message) => failure(message, ERROR_NOT_FOUND),
        })
    };
    unsafe { out.write(result) };
}

/// # Safety
/// `out` is return storage and `name` is a W# string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_ffi_symbol(out: *mut FallibleI64, library: i64, name: *const u8) {
    unsafe { crate::gc::checkpoint() };
    let name = unsafe { str_bytes(name) }.to_vec();
    let result = if name.is_empty() || name.contains(&0) {
        failure(
            "symbol name must be nonempty and contain no NUL",
            ERROR_BAD_FORMAT,
        )
    } else {
        crate::worker::blocking(|| {
            let held = resources()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .libraries
                .get(&library)
                .cloned();
            let Some(held) = held else {
                return failure("library is closed", ERROR_IO_FAILED);
            };
            match unsafe { loader::symbol(held.0, &name) } {
                Ok(address) => {
                    let mut table = resources().lock().unwrap_or_else(|e| e.into_inner());
                    if !table.libraries.contains_key(&library) {
                        return failure("library is closed", ERROR_IO_FAILED);
                    }
                    let id = table.id();
                    table.symbols.insert(id, (library, address));
                    FallibleI64::ok(id)
                }
                Err(message) => failure(message, ERROR_NOT_FOUND),
            }
        })
    };
    unsafe { out.write(result) };
}

#[unsafe(no_mangle)]
pub extern "C" fn ws_ffi_close(library: i64) {
    unsafe { crate::gc::checkpoint() };
    crate::worker::blocking(|| {
        let held = {
            let mut table = resources().lock().unwrap_or_else(|e| e.into_inner());
            table.symbols.retain(|_, (owner, _)| *owner != library);
            table.libraries.remove(&library)
        };
        // dlclose may run native destructors, so never hold our registry lock.
        drop(held);
    });
}

/// Called only by generated wrappers, with stack storage containing no roots.
/// # Safety
/// `thunk` must be a compiler-generated C thunk and `data` its scalar buffer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_ffi_call(
    symbol: i64,
    thunk: unsafe extern "C" fn(usize, *mut u8),
    data: *mut u8,
) {
    unsafe { crate::gc::checkpoint() };
    crate::worker::blocking(|| {
        let found = {
            let table = resources().lock().unwrap_or_else(|e| e.into_inner());
            table.symbols.get(&symbol).and_then(|(library, address)| {
                table
                    .libraries
                    .get(library)
                    .cloned()
                    .map(|held| (held, *address))
            })
        };
        let Some((_held, address)) = found else {
            crate::builtins::report_and_exit("FFI call uses a closed library or invalid binding");
        };
        unsafe { thunk(address, data) };
    });
}

fn allocate(size: i64, initial: &[u8]) -> FallibleI64 {
    let Ok(size) = usize::try_from(size) else {
        return failure("negative buffer size", ERROR_BAD_FORMAT);
    };
    let Some(words) = size.checked_add(7).map(|n| (n / 8).max(1)) else {
        return failure("buffer is too large", ERROR_BAD_FORMAT);
    };
    // Word storage guarantees alignment suitable for every supported scalar.
    let mut memory = Vec::<u64>::new();
    if memory.try_reserve_exact(words).is_err() {
        return failure("cannot allocate native buffer", ERROR_IO_FAILED);
    }
    memory.resize(words, 0);
    unsafe {
        std::ptr::copy_nonoverlapping(
            initial.as_ptr(),
            memory.as_mut_ptr().cast::<u8>(),
            initial.len(),
        )
    };
    let mut table = resources().lock().unwrap_or_else(|e| e.into_inner());
    let id = table.id();
    table.buffers.insert(id, memory.into_boxed_slice());
    table.sizes.insert(id, size);
    FallibleI64::ok(id)
}

/// # Safety
/// `out` is return storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_ffi_buffer(out: *mut FallibleI64, size: i64) {
    unsafe { crate::gc::checkpoint() };
    let result = crate::worker::blocking(|| allocate(size, &[]));
    unsafe { out.write(result) };
}

/// # Safety
/// `out` is return storage and `value` is a W# string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_ffi_c_string(out: *mut FallibleI64, value: *const u8) {
    unsafe { crate::gc::checkpoint() };
    let value = unsafe { str_bytes(value) }.to_vec();
    let result = if value.contains(&0) {
        failure("C string contains NUL", ERROR_BAD_FORMAT)
    } else {
        crate::worker::blocking(|| allocate(value.len() as i64 + 1, &value))
    };
    unsafe { out.write(result) };
}

/// # Safety
/// `out` is return storage (`!u64` has the same layout as `!i64`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_ffi_address(out: *mut FallibleI64, buffer: i64) {
    unsafe { crate::gc::checkpoint() };
    let result = crate::worker::blocking(|| {
        let mut table = resources().lock().unwrap_or_else(|e| e.into_inner());
        match table.buffers.get_mut(&buffer) {
            Some(bytes) => FallibleI64::ok(bytes.as_mut_ptr() as i64),
            None => failure("buffer is freed or invalid", ERROR_IO_FAILED),
        }
    });
    unsafe { out.write(result) };
}

fn bounds(offset: i64, size: i64, len: usize) -> Option<std::ops::Range<usize>> {
    let start = usize::try_from(offset).ok()?;
    let end = start.checked_add(usize::try_from(size).ok()?)?;
    (end <= len).then_some(start..end)
}

/// # Safety
/// `out` is return storage. The buffer must not be concurrently mutated by C.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_ffi_read(out: *mut FallibleStr, buffer: i64, offset: i64, size: i64) {
    unsafe { crate::gc::checkpoint() };
    let result = crate::worker::blocking(|| {
        let table = resources().lock().unwrap_or_else(|e| e.into_inner());
        let memory = table.buffers.get(&buffer).ok_or(ERROR_IO_FAILED)?;
        let range = bounds(offset, size, table.sizes[&buffer]).ok_or(ERROR_BAD_FORMAT)?;
        Ok(unsafe {
            std::slice::from_raw_parts(memory.as_ptr().cast::<u8>().add(range.start), range.len())
        }
        .to_vec())
    });
    let result = match result {
        Ok(bytes) => FallibleStr::ok(alloc_str(&bytes)),
        Err(tag) => FallibleStr::err(tag),
    };
    unsafe { out.write(result) };
}

/// # Safety
/// `value` is a W# string. Native users of the buffer must be synchronized.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_ffi_write(buffer: i64, offset: i64, value: *const u8) -> i64 {
    unsafe { crate::gc::checkpoint() };
    let value = unsafe { str_bytes(value) }.to_vec();
    crate::worker::blocking(|| {
        let mut table = resources().lock().unwrap_or_else(|e| e.into_inner());
        let Some(&len) = table.sizes.get(&buffer) else {
            return ERROR_IO_FAILED;
        };
        let Some(range) = bounds(offset, value.len() as i64, len) else {
            return ERROR_BAD_FORMAT;
        };
        let memory = table.buffers.get_mut(&buffer).unwrap();
        unsafe {
            std::ptr::copy_nonoverlapping(
                value.as_ptr(),
                memory.as_mut_ptr().cast::<u8>().add(range.start),
                range.len(),
            )
        };
        0
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn ws_ffi_free(buffer: i64) {
    unsafe { crate::gc::checkpoint() };
    crate::worker::blocking(|| {
        let mut table = resources().lock().unwrap_or_else(|e| e.into_inner());
        table.buffers.remove(&buffer);
        table.sizes.remove(&buffer);
    });
}

/// # Safety
/// `out` is return storage. `address` names at least `size` readable native bytes,
/// with no concurrent writes. It must never name a W# heap object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_ffi_read_pointer(out: *mut FallibleStr, address: u64, size: i64) {
    unsafe { crate::gc::checkpoint() };
    let result = if size < 0 || size as u64 > isize::MAX as u64 || (address == 0 && size != 0) {
        FallibleStr::err(ERROR_BAD_FORMAT)
    } else {
        let bytes = crate::worker::blocking(|| {
            if size == 0 {
                Vec::new()
            } else {
                unsafe { std::slice::from_raw_parts(address as *const u8, size as usize) }.to_vec()
            }
        });
        FallibleStr::ok(alloc_str(&bytes))
    };
    unsafe { out.write(result) };
}

pub(crate) fn builtins() -> Vec<Builtin> {
    const HANDLE: B = B::ErrUnion(&B::I64, &["BadFormat", "IoFailed", "NotFound"]);
    const BYTES: B = B::ErrUnion(&B::Str, &["BadFormat", "IoFailed"]);
    macro_rules! row {
        ($name:literal, $params:expr, $ret:expr, $f:ident) => {
            Builtin {
                module: "std/ffi",
                name: $name,
                params: $params,
                ret: $ret,
                link: stringify!($f),
                ptr: $f as *const u8,
            }
        };
    }
    vec![
        row!("raw_open", &[B::Str], HANDLE, ws_ffi_open),
        row!("raw_symbol", &[B::I64, B::Str], HANDLE, ws_ffi_symbol),
        row!("raw_close", &[B::I64], B::Void, ws_ffi_close),
        row!("last_error", &[], B::Str, ws_ffi_last_error),
        row!("raw_buffer", &[B::I64], HANDLE, ws_ffi_buffer),
        row!("raw_c_string", &[B::Str], HANDLE, ws_ffi_c_string),
        row!(
            "raw_address",
            &[B::I64],
            B::ErrUnion(&B::U64, &["IoFailed"]),
            ws_ffi_address
        ),
        row!("raw_read", &[B::I64, B::I64, B::I64], BYTES, ws_ffi_read),
        row!(
            "raw_write",
            &[B::I64, B::I64, B::Str],
            B::ErrUnion(&B::Void, &["BadFormat", "IoFailed"]),
            ws_ffi_write
        ),
        row!("raw_free", &[B::I64], B::Void, ws_ffi_free),
        row!(
            "raw_read_pointer",
            &[B::U64, B::I64],
            BYTES,
            ws_ffi_read_pointer
        ),
        Builtin {
            module: "std/ffi",
            name: "raw_bind",
            params: &[B::I64],
            ret: B::Var(0),
            link: INLINE,
            ptr: ws_ffi_call as *const u8,
        },
    ]
}
