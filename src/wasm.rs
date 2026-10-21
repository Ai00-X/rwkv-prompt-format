//! WASM 专属的最小分配器接口。
//!
//! 检查/修复本身直接复用 C ABI（`rwkv_pf_check` / `rwkv_pf_fix` /
//! `rwkv_pf_fix_text` / `rwkv_pf_free_string`），这里只补两件事：
//!
//! - 让 JS 能在线性内存里分配一块**零初始化**的输入缓冲区（末尾自带 NUL）；
//! - 用同一个分配器释放输入缓冲区（不能用 JS 侧内存）。
//!
//! 输出字符串仍由 `rwkv_pf_free_string` 释放，与 C ABI 契约一致。

/// 分配 `len` 字节（全 0），返回线性内存偏移。`len == 0` 返回 0。
///
/// 调用方必须用 [`rwkv_pf_wasm_free`] 以**相同的 len** 释放；若把该指针
/// 传给 `rwkv_pf_check`，长度应为 `utf8_len + 1`（最后 1 字节是 NUL）。
#[unsafe(no_mangle)]
pub extern "C" fn rwkv_pf_wasm_alloc(len: usize) -> *mut u8 {
    if len == 0 {
        return std::ptr::null_mut();
    }
    Box::into_raw(vec![0u8; len].into_boxed_slice()).cast::<u8>()
}

/// 释放 [`rwkv_pf_wasm_alloc`] 返回的缓冲区。NULL 是 no-op。
///
/// # Safety
/// `ptr` 必须来自 `rwkv_pf_wasm_alloc(len)`，且只释放一次。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rwkv_pf_wasm_free(ptr: *mut u8, len: usize) {
    if ptr.is_null() {
        return;
    }
    // SAFETY: 指针由 `Box<[u8]>` 以长度 `len` 泄漏而来。
    unsafe {
        drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)));
    }
}

/// ABI 版本。包装层用它拒绝加载过期的 `.wasm`，避免静默错位。
#[unsafe(no_mangle)]
pub extern "C" fn rwkv_pf_wasm_abi() -> u32 {
    1
}
