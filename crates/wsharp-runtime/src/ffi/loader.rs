//! The platform dynamic loader. All calls happen inside a safe region.

#[cfg(unix)]
mod platform {
    use std::ffi::{CStr, CString, c_char, c_int, c_void};
    #[cfg_attr(any(target_os = "linux", target_os = "android"), link(name = "dl"))]
    unsafe extern "C" {
        fn dlopen(path: *const c_char, flags: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
        fn dlclose(handle: *mut c_void) -> c_int;
        fn dlerror() -> *const c_char;
    }
    fn error() -> String {
        let message = unsafe { dlerror() };
        if message.is_null() {
            "dynamic loader failed".into()
        } else {
            unsafe { CStr::from_ptr(message) }
                .to_string_lossy()
                .into_owned()
        }
    }
    pub fn open(path: &[u8]) -> Result<usize, String> {
        let path = CString::new(path).map_err(|_| "library path contains NUL")?;
        // RTLD_NOW = 2. Darwin's RTLD_LOCAL is 4; other supported Unix targets
        // use zero. Local visibility avoids changing unrelated symbol lookups.
        let local = if cfg!(target_vendor = "apple") { 4 } else { 0 };
        let handle = unsafe { dlopen(path.as_ptr(), 2 | local) };
        if handle.is_null() {
            Err(error())
        } else {
            Ok(handle as usize)
        }
    }
    pub unsafe fn symbol(handle: usize, name: &[u8]) -> Result<usize, String> {
        let name = CString::new(name).map_err(|_| "symbol name contains NUL")?;
        unsafe { dlerror() };
        let address = unsafe { dlsym(handle as *mut c_void, name.as_ptr()) };
        let message = unsafe { dlerror() };
        if !message.is_null() {
            Err(unsafe { CStr::from_ptr(message) }
                .to_string_lossy()
                .into_owned())
        } else if address.is_null() {
            Err("symbol has a null address".into())
        } else {
            Ok(address as usize)
        }
    }
    pub unsafe fn close(handle: usize) {
        unsafe { dlclose(handle as *mut c_void) };
    }
}

#[cfg(windows)]
mod platform {
    use std::ffi::{CString, c_char, c_void};
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryW(path: *const u16) -> *mut c_void;
        fn GetProcAddress(handle: *mut c_void, name: *const c_char) -> *mut c_void;
        fn FreeLibrary(handle: *mut c_void) -> i32;
    }
    pub fn open(path: &[u8]) -> Result<usize, String> {
        let path = std::str::from_utf8(path).map_err(|_| "library path is not UTF-8")?;
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let handle = unsafe { LoadLibraryW(wide.as_ptr()) };
        if handle.is_null() {
            Err(std::io::Error::last_os_error().to_string())
        } else {
            Ok(handle as usize)
        }
    }
    pub unsafe fn symbol(handle: usize, name: &[u8]) -> Result<usize, String> {
        let name = CString::new(name).map_err(|_| "symbol name contains NUL")?;
        let address = unsafe { GetProcAddress(handle as *mut c_void, name.as_ptr()) };
        if address.is_null() {
            Err(std::io::Error::last_os_error().to_string())
        } else {
            Ok(address as usize)
        }
    }
    pub unsafe fn close(handle: usize) {
        unsafe { FreeLibrary(handle as *mut c_void) };
    }
}

pub(super) use platform::{close, open, symbol};
