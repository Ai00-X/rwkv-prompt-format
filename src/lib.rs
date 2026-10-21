//! RWKV 提示词格式检测与自动修复。
//!
//! 六种消费方式共用同一份 Rust 实现与同一份 JSON schema：
//!
//! 1. **Rust**：`rwkv_prompt_format::check` / `fix`；
//! 2. **CLI**：`rwkv-pf check|fix`（JSON 输出，任何语言都能起进程调用）；
//! 3. **C ABI**：`cdylib` / `staticlib` + `include/rwkv_prompt_format.h`，
//!    Python / Node / Go / Java / C# 的薄封装都在 `bindings/`；
//! 4. **WASM**：`wasm/rwkv_prompt_format.wasm` + `wasm/index.mjs`；
//! 5. **MCP**：`mcp/server.mjs`（零依赖 stdio，AI agent 直接调用）；
//! 6. **Skill**：`skills/rwkv-prompt-format/`，由 `scripts/install-agent.mjs`
//!    一键安装到 Qoder。
//!
//! 规则来源是 RWKV 官方 G1/G1x 模板（2026-10-21 逐字抓取），不是猜测：
//! `User: xxx\n\nAssistant:`、`System: xxx\n\nUser: xxx\n\nAssistant:`、
//! `Instruction: ...\n\nInput: ...\n\nResponse:`；`\n\n` 只作块分隔；标签、`<think`、
//! `(think)`、`Tools:` 均大小写敏感。详见 README。
//!
//! 已知边界：只检查/修复**语法与结构**，不判断内容是否正确；不计算 token 预算。

mod check;
mod ffi;
mod fix;
mod types;

#[cfg(target_arch = "wasm32")]
mod wasm;

pub use check::check;
pub use fix::{
    AppliedFix, FIX_SAFE, FIX_STRIP_METADATA, FIX_STRUCTURAL, FixOptions, FixResult, fix,
};
pub use types::{Finding, FormatReport, FormatStats, GenerationPoint, Severity};

/// crate 版本（和 C ABI 的 `rwkv_pf_version()` 一致）。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 检查并直接返回 JSON。任何语言都可以用这个 schema。
pub fn check_json(text: &str) -> String {
    serde_json::to_string(&check(text)).unwrap_or_else(|e| {
        serde_json::json!({ "error": format!("serialize report: {e}") }).to_string()
    })
}

/// 修复并直接返回 JSON（含 `before` / `after` / `applied` / `text`）。
pub fn fix_json(text: &str, options: &FixOptions) -> String {
    serde_json::to_string(&fix(text, options)).unwrap_or_else(|e| {
        serde_json::json!({ "error": format!("serialize fix result: {e}") }).to_string()
    })
}
