//! C ABI surface — everything declared in include/regkit.h.
//! Conventions (house standard):
//! - `*mut c_char` JSON results are caller-owned; free with
//!   `rg_free_string`.
//! - Failures return null (or the ERR_* code for
//!   `rg_apply_undo`) and set the thread-local last error.
//! - Nothing unwinds across the FFI boundary (catch_unwind
//!   everywhere; release profile also sets panic = "abort").

#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::panic::catch_unwind;

use crate::{ERR_INTERNAL, ERR_INVALID_ARG, RG_OK};

thread_local! {
    static LAST_ERROR: RefCell<(i32, CString)> =
        RefCell::new((0, CString::new("").unwrap()));
}

pub fn set_error(code: i32, msg: &str) {
    let clean: String = msg.chars().filter(|&c| c != '\0').collect();
    LAST_ERROR.with(|e| {
        *e.borrow_mut() = (
            code,
            CString::new(clean).unwrap_or_else(|_| CString::new("error").unwrap()),
        );
    });
}

fn clear_error() {
    set_error(RG_OK, "");
}

fn out_json(result: crate::RgResult<String>) -> *mut c_char {
    match result {
        Ok(text) => match CString::new(text) {
            Ok(s) => {
                clear_error();
                s.into_raw()
            }
            Err(_) => {
                set_error(ERR_INTERNAL, "result string contained a NUL byte");
                std::ptr::null_mut()
            }
        },
        Err((code, msg)) => {
            set_error(code, &msg);
            std::ptr::null_mut()
        }
    }
}

fn cstr<'a>(ptr: *const c_char, what: &str) -> crate::RgResult<&'a str> {
    if ptr.is_null() {
        return Err((ERR_INVALID_ARG, format!("{what} must not be null")));
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map_err(|_| (ERR_INVALID_ARG, format!("{what} must be valid UTF-8")))
}

fn guard_json<F: FnOnce() -> *mut c_char>(f: F) -> *mut c_char {
    match catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(p) => p,
        Err(_) => {
            set_error(ERR_INTERNAL, "internal panic caught at the FFI boundary");
            std::ptr::null_mut()
        }
    }
}

fn guard_code<F: FnOnce() -> c_int>(f: F) -> c_int {
    match catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(c) => c,
        Err(_) => {
            set_error(ERR_INTERNAL, "internal panic caught at the FFI boundary");
            ERR_INTERNAL as c_int
        }
    }
}

#[no_mangle]
pub extern "C" fn rg_version() -> *const c_char {
    static VERSION_C: &[u8] = b"0.1.0\0";
    VERSION_C.as_ptr() as *const c_char
}

#[no_mangle]
pub extern "C" fn rg_last_error() -> *const c_char {
    LAST_ERROR.with(|e| e.borrow().1.as_ptr())
}

#[no_mangle]
pub extern "C" fn rg_last_error_code() -> c_int {
    LAST_ERROR.with(|e| e.borrow().0) as c_int
}

/// # Safety (caller contract): `p` is null or a string this
/// library returned and has not been freed yet. Documented
/// in the header.
#[no_mangle]
pub extern "C" fn rg_free_string(p: *mut c_char) {
    if !p.is_null() {
        unsafe {
            drop(CString::from_raw(p));
        }
    }
}

#[no_mangle]
pub extern "C" fn rg_read(spec_json: *const c_char) -> *mut c_char {
    guard_json(|| {
        let inner = cstr(spec_json, "spec_json").and_then(crate::read);
        out_json(inner)
    })
}

#[no_mangle]
pub extern "C" fn rg_list(spec_json: *const c_char) -> *mut c_char {
    guard_json(|| {
        let inner = cstr(spec_json, "spec_json").and_then(crate::list);
        out_json(inner)
    })
}

#[no_mangle]
pub extern "C" fn rg_write(spec_json: *const c_char) -> *mut c_char {
    guard_json(|| {
        let inner = cstr(spec_json, "spec_json").and_then(crate::write);
        out_json(inner)
    })
}

#[no_mangle]
pub extern "C" fn rg_delete(spec_json: *const c_char) -> *mut c_char {
    guard_json(|| {
        let inner = cstr(spec_json, "spec_json").and_then(crate::delete);
        out_json(inner)
    })
}

#[no_mangle]
pub extern "C" fn rg_apply_undo(spec_json: *const c_char) -> c_int {
    guard_code(|| {
        let inner = cstr(spec_json, "spec_json").and_then(crate::apply_undo);
        match inner {
            Ok(()) => {
                clear_error();
                RG_OK as c_int
            }
            Err((code, msg)) => {
                set_error(code, &msg);
                code as c_int
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Only the off-Windows FFI test uses this helper; on
    // Windows it would be dead code.
    #[cfg(not(windows))]
    fn cs(s: &str) -> std::ffi::CString {
        std::ffi::CString::new(s).unwrap()
    }

    #[test]
    fn ffi_version_and_free() {
        let v = unsafe { CStr::from_ptr(rg_version()) }.to_str().unwrap();
        assert_eq!(v, "0.1.0");
        rg_free_string(std::ptr::null_mut()); // must be a no-op
    }

    #[test]
    fn ffi_null_spec_is_invalid_arg() {
        assert!(rg_read(std::ptr::null()).is_null());
        assert_eq!(rg_last_error_code(), ERR_INVALID_ARG);
        assert_eq!(rg_apply_undo(std::ptr::null()), ERR_INVALID_ARG as c_int);
        assert_eq!(rg_last_error_code(), ERR_INVALID_ARG);
    }

    /// Off Windows every registry call is a clean
    /// ERR_NOT_SUPPORTED (the FFI contract the smoke pack
    /// asserts on other hosts).
    #[cfg(not(windows))]
    #[test]
    fn ffi_ops_not_supported_off_windows() {
        let spec = cs(r#"{"root":"HKCU","path":"Software\\RegKit"}"#);
        assert!(rg_read(spec.as_ptr()).is_null());
        assert_eq!(rg_last_error_code(), crate::ERR_NOT_SUPPORTED);
        assert!(rg_list(spec.as_ptr()).is_null());
        assert_eq!(rg_last_error_code(), crate::ERR_NOT_SUPPORTED);
        let w =
            cs(r#"{"root":"HKCU","path":"Software\\RegKit","name":"X","type":"sz","value":"y"}"#);
        assert!(rg_write(w.as_ptr()).is_null());
        assert_eq!(rg_last_error_code(), crate::ERR_NOT_SUPPORTED);
        assert_eq!(rg_apply_undo(cs(r#"{"manifest":"{}"}"#).as_ptr()), {
            // The manifest is validated before the backend is
            // touched, so a malformed one is INVALID_ARG even
            // off Windows: "{}" has no version/entries.
            ERR_INVALID_ARG as c_int
        });
    }
}
