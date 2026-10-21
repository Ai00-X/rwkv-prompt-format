//! 对外数据结构。全部可 `serde_json` 序列化，CLI / C ABI / 各语言封装共用同一份 JSON。

use serde::Serialize;

/// 检测结论的严重级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warn,
    Info,
}

impl Severity {
    pub(crate) fn rank(self) -> u8 {
        match self {
            Self::Error => 0,
            Self::Warn => 1,
            Self::Info => 2,
        }
    }

    pub fn tag(self) -> &'static str {
        match self {
            Self::Error => "E",
            Self::Warn => "W",
            Self::Info => "I",
        }
    }
}

/// 一条检测结论。`line` 从 1 开始；`0` 表示整段文本级问题。
#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub code: &'static str,
    pub severity: Severity,
    pub line: usize,
    pub message: String,
    pub evidence: String,
}

/// 生成点：prompt 末尾等待模型续写的位置。
#[derive(Debug, Clone, Serialize)]
pub struct GenerationPoint {
    pub line: usize,
    pub role: String,
    /// true = 标签后已经 seed 了内容（如 `Assistant: {"answer": `、`English: A principios`）。
    pub seeded: bool,
}

/// 可核对的文本统计。
#[derive(Debug, Clone, Default, Serialize)]
pub struct FormatStats {
    pub chars: usize,
    pub lines: usize,
    pub user_blocks: usize,
    pub assistant_blocks: usize,
    pub system_blocks: usize,
    pub instruction_blocks: usize,
    pub input_blocks: usize,
    pub response_blocks: usize,
    /// 恰好一个空行的块间分隔数量（官方 `\n\n`）。
    pub blank_separators: usize,
    pub max_blank_run: usize,
    pub metadata_lines: usize,
    pub think_prefix: bool,
    pub function_call: bool,
}

/// 一次格式检查的完整产物（可直接序列化成 JSON）。
#[derive(Debug, Clone, Serialize)]
pub struct FormatReport {
    /// 规则档位。当前默认 checkpoint 是 RWKV-7 G1 系列，规则按 G1 官方模板检查。
    pub profile: &'static str,
    /// `plain_chat` / `material_qa` / `instruction` / `instruction_chat` /
    /// `system_tools` / `chatml` / `markdown` / `jsonl` / `continuation` / `unknown`
    pub format: &'static str,
    pub ok: bool,
    pub error_count: usize,
    pub warning_count: usize,
    pub info_count: usize,
    pub generation_point: Option<GenerationPoint>,
    pub stats: FormatStats,
    pub findings: Vec<Finding>,
}

impl FormatReport {
    /// 一行摘要，供 `prompt` / `ask` 的日志使用。
    pub fn summary_line(&self) -> String {
        let status = if self.ok { "OK" } else { "FAIL" };
        let gp = self
            .generation_point
            .as_ref()
            .map(|g| {
                format!(
                    "L{} {}{}",
                    g.line,
                    g.role,
                    if g.seeded { "(seed)" } else { "" }
                )
            })
            .unwrap_or_else(|| "无".to_string());
        format!(
            "{}（{} error / {} warn）· 格式 {} · 生成点 {}",
            status, self.error_count, self.warning_count, self.format, gp
        )
    }
}
