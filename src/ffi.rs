//! 稳定 C ABI。所有字符串都是 UTF-8、NUL 结尾；返回的堆字符串必须用
//! `rwkv_pf_free_string` 释放（不能用 `free()`，分配器不同）。
//!
//! 线程安全：所有函数都是纯函数，可并发调用。

use std::ffi::{CStr, CString, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::check::check;
use crate::fix::{FixOptions, fix};

/// 与 `Cargo.toml` 一致的版本号（静态字符串，**不要释放**）。
const VERSION_C: &str = concat!(env!("CARGO_PKG_VERSION"), "\0");

/// 返回检查器版本，例如 `"0.1.0"`。指针指向静态内存，不要释放。
#[unsafe(no_mangle)]
pub extern "C" fn rwkv_pf_version() -> *const c_char {
    VERSION_C.as_ptr().cast()
}

/// 检查一段 prompt，返回 `FormatReport` 的 JSON。调用方负责 `rwkv_pf_free_string`。
///
/// - `text_utf8` 必须是有效的 UTF-8 C 字符串；
/// - 出错时返回 `{"error":"..."}` 而不是 NULL（方便封装统一处理）。
///
/// # Safety
/// `text_utf8` 必须指向一个以 NUL 结尾的可读字符串，或在调用期间保持有效。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rwkv_pf_check(text_utf8: *const c_char) -> *mut c_char {
    respond_json(|| {
        let text = read_cstr(text_utf8)?;
        serde_json::to_string(&check(&text)).map_err(|e| format!("serialize report: {e}"))
    })
}

/// 修复一段 prompt，返回 `FixResult` 的 JSON（含 `text` / `before` / `after` /
/// `applied` / `notes`）。`flags` 见头文件里的 `RWKV_PF_FIX_*`。
///
/// # Safety
/// 同 [`rwkv_pf_check`]。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rwkv_pf_fix(text_utf8: *const c_char, flags: u32) -> *mut c_char {
    respond_json(|| {
        let text = read_cstr(text_utf8)?;
        let result = fix(&text, &FixOptions::from_flags(flags));
        serde_json::to_string(&result).map_err(|e| format!("serialize fix result: {e}"))
    })
}

/// 只返回修复后的文本（不包 JSON）。失败（NULL / 非 UTF-8 / panic）返回 NULL。
///
/// # Safety
/// 同 [`rwkv_pf_check`]。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rwkv_pf_fix_text(text_utf8: *const c_char, flags: u32) -> *mut c_char {
    respond_text(|| {
        let text = read_cstr(text_utf8)?;
        Ok(fix(&text, &FixOptions::from_flags(flags)).text)
    })
}

/// 释放 [`rwkv_pf_check`] / [`rwkv_pf_fix`] / [`rwkv_pf_fix_text`] 返回的字符串。
/// NULL 是合法的 no-op。
///
/// # Safety
/// `ptr` 必须来自本库，且只能释放一次。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rwkv_pf_free_string(ptr: *mut c_char) {
    if ptr.is_null() {
        return;
    }
    // SAFETY: 指针由 `CString::into_raw` 产生，调用方承诺只释放一次。
    unsafe {
        drop(CString::from_raw(ptr));
    }
}

fn read_cstr(ptr: *const c_char) -> Result<String, String> {
    if ptr.is_null() {
        return Err("null input".to_string());
    }
    // SAFETY: 调用方承诺指针指向 NUL 结尾的可读字符串。
    let c = unsafe { CStr::from_ptr(ptr) };
    c.to_str()
        .map(str::to_owned)
        .map_err(|_| "input is not valid UTF-8".to_string())
}

fn respond_json(f: impl FnOnce() -> Result<String, String>) -> *mut c_char {
    let text = match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(json)) => json,
        Ok(Err(err)) => error_json(&err),
        Err(_) => error_json("internal panic"),
    };
    to_c_string(text)
}

fn respond_text(f: impl FnOnce() -> Result<String, String>) -> *mut c_char {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(text)) => to_c_string(text),
        _ => std::ptr::null_mut(),
    }
}

fn error_json(message: &str) -> String {
    serde_json::json!({ "error": message }).to_string()
}

fn to_c_string(text: String) -> *mut c_char {
    match CString::new(text) {
        Ok(c) => c.into_raw(),
        Err(_) => CString::new("{\"error\":\"NUL byte in output\"}")
            .expect("static JSON has no NUL")
            .into_raw(),
    }
}
