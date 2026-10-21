//! 检测核心：纯文本解析 + RWKV G1/G1x 官方格式规则。
//!
//! 不加载模型、不读写磁盘，输入输出都是字符串。

use crate::types::{Finding, FormatReport, FormatStats, GenerationPoint, Severity};

// ---------------------------------------------------------------------------
// 内部：角色标签与块结构
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Label {
    User,
    Assistant,
    System,
    Instruction,
    Input,
    Response,
}

pub(crate) const KNOWN: [(&str, Label); 6] = [
    ("Assistant", Label::Assistant),
    ("Instruction", Label::Instruction),
    ("Response", Label::Response),
    ("System", Label::System),
    ("Input", Label::Input),
    ("User", Label::User),
];

pub(crate) fn label_name(label: Label) -> &'static str {
    match label {
        Label::User => "User",
        Label::Assistant => "Assistant",
        Label::System => "System",
        Label::Instruction => "Instruction",
        Label::Input => "Input",
        Label::Response => "Response",
    }
}

/// 只在**行首**识别官方标签：`User:` / `Assistant:` / `System:` /
/// `Instruction:` / `Input:` / `Response:`。返回标签与同一行剩余部分。
pub(crate) fn parse_label(line: &str) -> Option<(Label, &str)> {
    for (name, label) in KNOWN {
        if let Some(rest) = line.strip_prefix(name)
            && let Some(rest) = rest.strip_prefix(':')
        {
            return Some((label, rest));
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Malformed {
    Indented,
    Case,
    FullWidthColon,
    SpaceBeforeColon,
}

/// 识别「看起来像角色标签但写法不对」的行。合法行返回 `None`。
///
/// 标签**大小写敏感**：`user:` / `USER:` / `Assistant:` 的任意大小写变体都算错。
/// 标签与冒号之间也不允许空格/Tab（`User :`、`User\t:`），全角冒号同样算错。
pub(crate) fn malformed_label(line: &str) -> Option<(Malformed, &'static str)> {
    let trimmed = line.trim_start();
    if trimmed.is_empty() {
        return None;
    }
    let indented = trimmed.len() != line.len();
    for (name, _) in KNOWN {
        let Some(rest) = trimmed.get(name.len()..) else {
            continue;
        };
        let head = &trimmed[..name.len()];
        if !head.eq_ignore_ascii_case(name) {
            continue;
        }
        // 冒号前允许先出现空白，才能把 `USER :` / `User ：` / `User\t:` 一并识别。
        let after_space = rest.trim_start_matches([' ', '\t']);
        let full_width = after_space.starts_with(['：', '﹕', '︓']);
        let half_width = after_space.starts_with(':');
        if !(full_width || half_width) {
            continue;
        }
        let has_space = after_space.len() != rest.len();
        if head != name {
            return Some((Malformed::Case, name));
        }
        if full_width {
            return Some((Malformed::FullWidthColon, name));
        }
        if has_space {
            return Some((Malformed::SpaceBeforeColon, name));
        }
        if indented {
            return Some((Malformed::Indented, name));
        }
        return None;
    }
    None
}

#[derive(Debug, Clone)]
struct Block {
    label: Label,
    /// 1-based 行号。
    line: usize,
    /// 标签同一行、冒号之后的内容（保留前导空格用于语法检查）。
    rest: String,
    /// 标签行之后到下一个标签之前的行下标（含块间空行）。
    body: Vec<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    PlainChat,
    MaterialQa,
    Instruction,
    InstructionChat,
    SystemTools,
    Chatml,
    Markdown,
    Jsonl,
    Continuation,
    Unknown,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Self::PlainChat => "plain_chat",
            Self::MaterialQa => "material_qa",
            Self::Instruction => "instruction",
            Self::InstructionChat => "instruction_chat",
            Self::SystemTools => "system_tools",
            Self::Chatml => "chatml",
            Self::Markdown => "markdown",
            Self::Jsonl => "jsonl",
            Self::Continuation => "continuation",
            Self::Unknown => "unknown",
        }
    }

    fn foreign(self) -> bool {
        matches!(self, Self::Chatml | Self::Markdown | Self::Jsonl)
    }
}

pub(crate) fn strip_cr(line: &str) -> &str {
    line.strip_suffix('\r').unwrap_or(line)
}

fn evidence(line: &str) -> String {
    let line = strip_cr(line);
    let mut out: String = line.chars().take(120).collect();
    if line.chars().count() > 120 {
        out.push('…');
    }
    out.replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\r', "\\r")
}

fn line_at(text: &str, idx: usize) -> String {
    let start = text[..idx].rfind('\n').map_or(0, |p| p + 1);
    let end = text[idx..].find('\n').map_or(text.len(), |p| idx + p);
    evidence(&text[start..end])
}

fn line_of(text: &str, idx: usize) -> usize {
    text[..idx].matches('\n').count() + 1
}

fn block_text(lines: &[&str], block: &Block) -> String {
    let mut out = String::from(block.rest.trim_start());
    for &i in &block.body {
        let line = strip_cr(lines[i]);
        if line.trim().is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line);
    }
    out
}

fn has_content(lines: &[&str], block: &Block) -> bool {
    !block.rest.trim().is_empty()
        || block
            .body
            .iter()
            .any(|i| !strip_cr(lines[*i]).trim().is_empty())
}

fn trailing_blank(lines: &[&str], body: &[usize]) -> usize {
    body.iter()
        .rev()
        .take_while(|i| strip_cr(lines[**i]).trim().is_empty())
        .count()
}

/// 标签行 + 后续各行的**原始**拼接（保留缩进、空行与行尾空格）。
/// 用于需要逐字比对的场合（think 前缀、尾部空白），不要用会 trim 的 `block_text`。
fn raw_block(lines: &[&str], block: &Block) -> String {
    let mut out = block.rest.clone();
    for &i in &block.body {
        out.push('\n');
        out.push_str(strip_cr(lines[i]));
    }
    out
}

fn body_has_content(lines: &[&str], block: &Block) -> bool {
    block
        .body
        .iter()
        .any(|i| !strip_cr(lines[*i]).trim().is_empty())
}

/// 校验一个 Assistant 块开头的 `<think` 前缀（官方三种逐字形式）。
///
/// 历史轮次里的 think 前缀同样要查：只要块内容以 `<think` / `< think` 开头，
/// 格式错了就会改变模型对后续文本的解析。返回是否为合法 think 形式；
/// 非 think（普通 seeded 回答）返回 `false` 且不产生 finding。
fn check_think_prefix(
    lines: &[&str],
    block: &Block,
    push: &mut dyn FnMut(&'static str, Severity, usize, String, String),
    announce_valid: bool,
) -> bool {
    let raw = raw_block(lines, block);
    let raw = raw.trim_end_matches(['\r', '\n']);
    let lead = raw.trim_start();
    let low = lead.to_ascii_lowercase();
    if !(low.starts_with("<think") || low.starts_with("< think")) {
        return false;
    }

    // 官方逐字：`Assistant: <think` / `Assistant: <think></think>` /
    // `Assistant: <think>\n</think`。冒号后恰好一个半角空格。
    if raw.starts_with("  ") {
        push(
            "THINK_PREFIX_EXTRA_SPACE",
            Severity::Error,
            block.line,
            "`Assistant:` 与 `<think` 之间多于一个空格；官方是 `Assistant: <think`".to_string(),
            evidence(lines[block.line - 1]),
        );
    } else if raw.starts_with('\t') {
        push(
            "THINK_PREFIX_SPACING",
            Severity::Error,
            block.line,
            "`Assistant:` 与 `<think` 之间是 Tab；官方用一个半角空格".to_string(),
            evidence(lines[block.line - 1]),
        );
    } else if raw.starts_with(['\u{3000}', '\u{00a0}']) {
        push(
            "THINK_PREFIX_SPACING",
            Severity::Error,
            block.line,
            "`Assistant:` 与 `<think` 之间是全角/不换行空格；官方用一个半角空格".to_string(),
            evidence(lines[block.line - 1]),
        );
    } else if raw.starts_with('\n') || raw.starts_with('\r') {
        push(
            "THINK_PREFIX_NEWLINE",
            Severity::Warn,
            block.line,
            "`<think` 应在 `Assistant:` 同一行；官方是 `Assistant: <think`".to_string(),
            evidence(lines[block.line - 1]),
        );
    } else if !raw.starts_with(' ') {
        push(
            "THINK_PREFIX_NO_SPACE",
            Severity::Error,
            block.line,
            "`Assistant:` 后没有空格就直接接 `<think`；官方是 `Assistant: <think`".to_string(),
            evidence(lines[block.line - 1]),
        );
    }
    let mut seed = if raw.starts_with(' ') && !raw.starts_with("  ") {
        &raw[1..]
    } else {
        raw.trim_start()
    };
    if seed.starts_with('\n') || seed.starts_with('\r') {
        push(
            "THINK_PREFIX_NEWLINE",
            Severity::Warn,
            block.line,
            "`<think` 出现在 `Assistant:` 的下一行；官方把它放在同一行".to_string(),
            evidence(lines[block.line - 1]),
        );
        seed = seed.trim_start();
    }
    let trailing_space = seed.ends_with(' ') || seed.ends_with('\t') || seed.ends_with('\u{3000}');
    if trailing_space {
        push(
            "THINK_PREFIX_TRAILING_SPACE",
            Severity::Error,
            block.line,
            "`<think` 后还有尾随空白；官方三个形式都以 `<think` / `</think` 结束".to_string(),
            format!("{}␠", evidence(lines[block.line - 1])),
        );
    }
    let valid = matches!(seed, "<think" | "<think></think" | "<think>\n</think");
    // 大小写敏感：形态正确但字母大小写不对，单独报，避免只给一句笼统的 malformed。
    let core = seed.trim_end_matches([' ', '\t', '\u{3000}']);
    let lower = core.to_ascii_lowercase();
    let case_only = core != lower
        && matches!(
            lower.as_str(),
            "<think" | "<think></think" | "<think>\n</think"
        );
    if valid {
        if announce_valid {
            push(
                "THINK_MODE",
                Severity::Info,
                block.line,
                "think 前缀符合官方三种形式之一".to_string(),
                evidence(lines[block.line - 1]),
            );
        }
    } else if case_only {
        push(
            "THINK_PREFIX_CASE",
            Severity::Error,
            block.line,
            "think 前缀大小写敏感：`<think` 与 `</think` 必须全小写".to_string(),
            evidence(lines[block.line - 1]),
        );
    } else if !trailing_space {
        push(
            "THINK_MALFORMED",
            Severity::Error,
            block.line,
            "think 前缀不符合官方形式（只允许 `Assistant: <think`、\
             `Assistant: <think></think>`、`Assistant: <think>\\n</think`）"
                .to_string(),
            evidence(lines[block.line - 1]),
        );
    }
    valid
}

/// `System: Tools:` 是官方 G1x function call 模板的固定字面量：
/// 大小写敏感，`Tools` 与冒号之间不能有空格，冒号必须是半角。
fn check_tools_label(
    lines: &[&str],
    block: &Block,
    push: &mut impl FnMut(&'static str, Severity, usize, String, String),
) {
    let (line_no, line_text) = if !block.rest.trim().is_empty() {
        (block.line, block.rest.trim_start())
    } else {
        match block
            .body
            .iter()
            .find(|i| !strip_cr(lines[**i]).trim().is_empty())
        {
            Some(&i) => (i + 1, strip_cr(lines[i]).trim_start()),
            None => return,
        }
    };
    let Some(head) = line_text.get(..5) else {
        return;
    };
    if !head.eq_ignore_ascii_case("tools") {
        return;
    }
    let after = &line_text[5..];
    let after_space = after.trim_start_matches([' ', '\t']);
    let full_width = after_space.starts_with(['：', '﹕', '︓']);
    let half_width = after_space.starts_with(':');
    if !(full_width || half_width) {
        return;
    }
    let has_space = after_space.len() != after.len();
    let (code, msg) = if head != "Tools" {
        (
            "TOOLS_LABEL_CASE",
            "function call 的 `Tools:` 大小写敏感，官方是 `Tools:`".to_string(),
        )
    } else if full_width {
        (
            "TOOLS_LABEL_FULLWIDTH_COLON",
            "`Tools:` 必须用半角冒号".to_string(),
        )
    } else if has_space {
        (
            "TOOLS_LABEL_SPACE_BEFORE_COLON",
            "`Tools` 与冒号之间不能有空格；官方是 `Tools:`".to_string(),
        )
    } else {
        return;
    };
    push(code, Severity::Error, line_no, msg, evidence(line_text));
}

fn markdown_role(line: &str) -> Option<&'static str> {
    let t = line.trim_start();
    // 必须先有 Markdown 标记；否则裸的 `User:` 是官方 plain 格式，不是 Markdown。
    // `- User:` 不算角色头：列表项在 prompt 里很常见，误报会逼用户改正文。
    if !(t.starts_with('#') || t.starts_with("**") || t.starts_with('*') || t.starts_with("> ")) {
        return None;
    }
    let t = t.trim_start_matches('#').trim_start();
    let t = t.trim_start_matches(['*', '>', ' ']);
    for (name, _) in KNOWN {
        let Some(rest) = t.strip_prefix(name) else {
            continue;
        };
        if rest.is_empty() || rest.starts_with(':') || rest.starts_with('*') {
            return Some(name);
        }
    }
    None
}

fn jsonl_role(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("{\"role\"") && t.contains("\"content\"")
}

pub(crate) fn continuation_label(line: &str) -> Option<(&str, &str)> {
    const LABELS: [&str; 12] = [
        "English",
        "Chinese",
        "Japanese",
        "Korean",
        "French",
        "German",
        "Spanish",
        "Russian",
        "Portuguese",
        "Italian",
        "Arabic",
        "Hindi",
    ];
    let t = line.trim();
    if t.is_empty() {
        return None;
    }
    let (head, tail) = t.split_once(':')?;
    let head = head.trim();
    if !LABELS.iter().any(|l| l.eq_ignore_ascii_case(head)) {
        return None;
    }
    Some((head, tail))
}

/// 末尾的续写标签：`English:` / `English: A principios`。
///
/// 只认「整段文本的第一行」或「一个空行之后」的标签——否则 Assistant 正文里
/// 恰好以 `English:` 开头的一行会被误当生成点。返回 `(0-based 行号, 规范写法, seed)`。
pub(crate) fn continuation_tail<'a>(
    lines: &[&'a str],
    in_fence: &[bool],
) -> Option<(usize, &'static str, &'a str)> {
    let (idx, raw) = lines
        .iter()
        .enumerate()
        .rev()
        .find(|(_, raw)| !strip_cr(raw).trim().is_empty())?;
    if in_fence[idx] {
        return None;
    }
    let first_or_after_blank = lines[..idx]
        .iter()
        .all(|line| strip_cr(line).trim().is_empty())
        || strip_cr(lines[idx - 1]).trim().is_empty();
    if !first_or_after_blank {
        return None;
    }
    let (head, tail) = continuation_label(raw)?;
    let canonical = [
        "English",
        "Chinese",
        "Japanese",
        "Korean",
        "French",
        "German",
        "Spanish",
        "Russian",
        "Portuguese",
        "Italian",
        "Arabic",
        "Hindi",
    ]
    .into_iter()
    .find(|label| label.eq_ignore_ascii_case(head))?;
    Some((idx, canonical, tail))
}

fn fence_scan(lines: &[&str]) -> (Vec<bool>, Option<usize>) {
    let mut in_fence = vec![false; lines.len()];
    let mut fence = false;
    let mut opened_at = None;
    for (i, raw) in lines.iter().enumerate() {
        let line = strip_cr(raw);
        let fence_line = line.trim_start().starts_with("```");
        // 官方模板允许围栏直接跟在标签后：`Assistant: ```json`。
        // 这种标签行本身仍是块边界，但会开启围栏。
        let label_fence = !fence
            && parse_label(line).is_some_and(|(_, rest)| rest.trim_start().starts_with("```"));
        if fence_line {
            in_fence[i] = true;
            fence = !fence;
            opened_at = fence.then_some(i);
        } else if label_fence {
            in_fence[i] = false;
            fence = true;
            opened_at = Some(i);
        } else {
            in_fence[i] = fence;
        }
    }
    (in_fence, opened_at)
}

/// 标记每一行是否位于 Markdown 代码围栏内。
pub(crate) fn fence_flags(lines: &[&str]) -> Vec<bool> {
    fence_scan(lines).0
}

/// 最后一处没有闭合的围栏起始行（0-based）。
pub(crate) fn unclosed_fence(lines: &[&str]) -> Option<usize> {
    fence_scan(lines).1
}

fn foreign_markers() -> &'static [(&'static str, &'static str)] {
    &[
        ("<|im_start|>", "ChatML"),
        ("<|im_end|>", "ChatML"),
        ("<|start_header_id|>", "Llama 3"),
        ("<|end_header_id|>", "Llama 3"),
        ("<|eot_id|>", "Llama 3"),
        ("[INST]", "Llama 2"),
        ("<<SYS>>", "Llama 2"),
        ("<|endoftext|>", "GPT 系"),
    ]
}

/// 检查 User 块里的 `(think)` / `(think a bit)` / `(think a lot)`。
///
/// 官方逐字是 `"User: xxx (think)\n\nAssistant: <think"`：
/// 半角括号、`(` 前恰好一个半角空格、hint 必须在用户文本末尾。
/// 全角括号、`( think)`、`(think )`、`(Think)`、句中位置都会让触发条件失效。
fn check_think_hints(
    lines: &[&str],
    block: &Block,
    push: &mut impl FnMut(&'static str, Severity, usize, String, String),
) {
    let mut content: Vec<(usize, &str)> = Vec::new();
    if !block.rest.trim().is_empty() {
        content.push((block.line, block.rest.as_str()));
    }
    for &i in &block.body {
        let line = strip_cr(lines[i]);
        if !line.trim().is_empty() {
            content.push((i + 1, line));
        }
    }
    let Some(&(last_line, last_text)) = content.last() else {
        return;
    };
    let mut valid = 0usize;
    for &(line_no, text) in &content {
        let chars: Vec<char> = text.chars().collect();
        let mut idx = 0usize;
        while idx < chars.len() {
            let open = chars[idx];
            if open != '(' && open != '（' {
                idx += 1;
                continue;
            }
            let mut close = None;
            let mut j = idx + 1;
            while j < chars.len() && j <= idx + 48 {
                if chars[j] == ')' || chars[j] == '）' {
                    close = Some(j);
                    break;
                }
                j += 1;
            }
            let Some(close) = close else {
                idx += 1;
                continue;
            };
            let inner: String = chars[idx + 1..close].iter().collect();
            if !inner.to_ascii_lowercase().contains("think") {
                idx = close + 1;
                continue;
            }
            let half_width = open == '(' && chars[close] == ')';
            let exact = matches!(inner.as_str(), "think" | "think a bit" | "think a lot");
            let mut spaces = 0usize;
            let mut k = idx;
            while k > 0 && chars[k - 1] == ' ' {
                spaces += 1;
                k -= 1;
            }
            let tail: String = chars[close + 1..].iter().collect();
            let at_end = line_no == last_line && tail.trim().is_empty();
            if half_width && exact {
                valid += 1;
                if spaces != 1 {
                    push(
                        "THINK_HINT_SPACING",
                        Severity::Error,
                        line_no,
                        format!(
                            "`({inner})` 前应有且仅有一个半角空格（当前 {spaces} 个）；\
                             官方逐字是 `User: xxx (think)`"
                        ),
                        evidence(text),
                    );
                }
                if !at_end {
                    push(
                        "THINK_HINT_NOT_AT_END",
                        Severity::Warn,
                        line_no,
                        format!(
                            "`({inner})` 必须位于 User 文本末尾；官方示例是 `User: xxx (think)`，\
                             后面直接空行接 `Assistant:`"
                        ),
                        evidence(text),
                    );
                }
            } else if half_width && inner != inner.to_ascii_lowercase() {
                push(
                    "THINK_HINT_CASE",
                    Severity::Error,
                    line_no,
                    format!(
                        "`({inner})` 大小写敏感，必须写小写 `think`；\
                         官方只允许 `(think)` / `(think a bit)` / `(think a lot)`"
                    ),
                    evidence(text),
                );
            } else {
                let why = if !half_width {
                    "必须用半角括号 `(` `)`"
                } else {
                    "括号内只允许 `think` / `think a bit` / `think a lot`，且单词间各一个空格"
                };
                push(
                    "THINK_HINT_MALFORMED",
                    Severity::Error,
                    line_no,
                    format!("`({inner})` 不是官方思考长度 hint：{why}"),
                    evidence(text),
                );
            }
            idx = close + 1;
        }
    }
    if valid > 0 {
        push(
            "THINK_LENGTH_HINT",
            Severity::Info,
            last_line,
            "User 文本末尾含官方思考长度控制（`(think)` / `(think a bit)` / `(think a lot)`）"
                .to_string(),
            evidence(last_text),
        );
    }
    if valid > 1 {
        push(
            "THINK_HINT_MULTIPLE",
            Severity::Warn,
            last_line,
            format!("同一个 User 块里有 {valid} 个思考长度 hint；只需最后一个"),
            evidence(last_text),
        );
    }
}

// ---------------------------------------------------------------------------
// 主入口
// ---------------------------------------------------------------------------

/// 检查一段**已经拼好的 prompt**。不加载模型、不读写磁盘。
pub fn check(text: &str) -> FormatReport {
    let chars = text.chars().count();
    let lines: Vec<&str> = text.split('\n').collect();
    let line_count = if text.ends_with('\n') {
        lines.len().saturating_sub(1)
    } else {
        lines.len()
    };
    let mut stats = FormatStats {
        chars,
        lines: line_count,
        ..FormatStats::default()
    };
    let mut findings: Vec<Finding> = Vec::new();

    let mut push =
        |code: &'static str, severity: Severity, line: usize, message: String, ev: String| {
            findings.push(Finding {
                code,
                severity,
                line,
                message,
                evidence: ev,
            });
        };

    if text.trim().is_empty() {
        push(
            "EMPTY_PROMPT",
            Severity::Error,
            1,
            "prompt 为空".to_string(),
            String::new(),
        );
        return finish(Kind::Unknown, findings, stats, None);
    }

    // ---- 全局字节/字符问题 ----
    if text.starts_with('\u{feff}') {
        push(
            "BOM",
            Severity::Error,
            1,
            "prompt 以 BOM 开头；官方模板从 `User:` / `System:` 直接开始".to_string(),
            line_at(text, 0),
        );
    }
    if let Some(idx) = text.find("\r\n") {
        push(
            "CRLF",
            Severity::Warn,
            line_of(text, idx),
            "包含 CRLF；G1x 官方预处理先把 `\\r\\n` 换成 `\\n`".to_string(),
            line_at(text, idx),
        );
    }
    if !text.starts_with('\u{feff}') && text.chars().next().is_some_and(|c| c.is_whitespace()) {
        push(
            "LEADING_WHITESPACE",
            Severity::Warn,
            1,
            "prompt 以空白开头；第一个 token 应是角色标签".to_string(),
            line_at(text, 0),
        );
    }

    // ---- 非 RWKV 模板标记 ----
    let mut chatml = false;
    let mut jsonl = false;
    let mut markdown = false;
    for &(marker, family) in foreign_markers() {
        for (hits, (idx, _)) in text.match_indices(marker).enumerate() {
            if hits >= 3 {
                break;
            }
            if marker.starts_with("<|im_") {
                chatml = true;
            }
            push(
                "FOREIGN_MARKER",
                Severity::Error,
                line_of(text, idx),
                format!("`{marker}` 是 {family} 模板标记，不是 RWKV 训练格式"),
                line_at(text, idx),
            );
        }
    }

    let mut jsonl_hits = 0usize;
    let mut md_hits = 0usize;
    for (i, raw) in lines.iter().enumerate() {
        let line = strip_cr(raw);
        let trimmed = line.trim_start();
        if jsonl_role(line) {
            jsonl = true;
            if jsonl_hits < 3 {
                jsonl_hits += 1;
                push(
                    "JSONL_ROLE",
                    Severity::Error,
                    i + 1,
                    "`{\"role\":...}` 是 OpenAI/JSONL 模板，不是 RWKV 训练格式".to_string(),
                    evidence(line),
                );
            }
        }
        if markdown_role(line).is_some() {
            markdown = true;
            if md_hits < 6 {
                md_hits += 1;
                push(
                    "MARKDOWN_ROLE",
                    Severity::Error,
                    i + 1,
                    "Markdown 角色头不在 RWKV 训练格式里；官方只有 `User:` / `Assistant:`"
                        .to_string(),
                    evidence(line),
                );
            }
        }
        if trimmed.starts_with("Human:") || trimmed.starts_with("AI:") {
            push(
                "HUMAN_AI_ROLE",
                Severity::Error,
                i + 1,
                "`Human:` / `AI:` 不是 RWKV 角色标签".to_string(),
                evidence(line),
            );
        } else if trimmed.starts_with("Question:")
            || trimmed.starts_with("Answer:")
            || trimmed.starts_with("Q:")
            || trimmed.starts_with("A:")
        {
            push(
                "LEGACY_ROLE",
                Severity::Warn,
                i + 1,
                "`Question:/Answer:` / `Q:/A:` 是旧格式；RWKV-7 官方默认 `User:/Assistant:`"
                    .to_string(),
                evidence(line),
            );
        }
    }

    // 整份文档的 JSON（数组或 `{"messages":[…]}`）也是外部模板；逐行扫描
    // 只认 `{"role"` 开头的紧凑 JSONL，覆盖不了这两种常见写法。
    if !jsonl && let Ok(value) = serde_json::from_str::<serde_json::Value>(text.trim()) {
        let looks_like_messages = value
            .as_array()
            .is_some_and(|items| items.iter().any(|v| v.get("role").is_some()))
            || value
                .get("messages")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|items| items.iter().any(|v| v.get("role").is_some()));
        if looks_like_messages {
            jsonl = true;
            push(
                "JSONL_ROLE",
                Severity::Error,
                1,
                "整份 JSON 消息数组是 OpenAI/JSONL 模板，不是 RWKV 训练格式".to_string(),
                evidence(lines[0]),
            );
        }
    }

    // ---- 代码围栏：围栏内的 `User:` 是内容，不是块边界 ----
    let in_fence = fence_flags(&lines);
    if let Some(idx) = unclosed_fence(&lines) {
        push(
            "UNCLOSED_CODE_FENCE",
            Severity::Error,
            idx + 1,
            "代码围栏没有闭合；后面的角色标签会被当成围栏内容，模型看到的结构会变".to_string(),
            evidence(lines[idx]),
        );
    }

    // 续写标签是独立生成点，不能当成上一块正文。外语模板里出现的 `English:`
    // 不算（那些格式整体就不合法，由各自的 finding 处理）。
    let continuation = if chatml || jsonl || markdown {
        None
    } else {
        continuation_tail(&lines, &in_fence)
    };
    let parse_end = continuation.map_or(lines.len(), |(idx, _, _)| idx);

    // ---- 块解析 ----
    let mut blocks: Vec<Block> = Vec::new();
    let mut current: Option<Block> = None;
    let mut preamble: Vec<usize> = Vec::new();
    for (i, raw) in lines.iter().enumerate().take(parse_end) {
        let line = strip_cr(raw);

        if let Some((bad, name)) = malformed_label(line) {
            let (code, msg) = match bad {
                Malformed::Indented => (
                    "LABEL_INDENTED",
                    format!("角色标签 `{name}:` 不能缩进；必须顶格"),
                ),
                Malformed::Case => (
                    "LABEL_CASE",
                    format!(
                        "角色标签大小写敏感：`{}` 不是官方写法；必须是 `{name}:`（首字母大写，其余小写）",
                        line.trim_start()
                    ),
                ),
                Malformed::FullWidthColon => (
                    "LABEL_FULLWIDTH_COLON",
                    format!("角色标签用了全角冒号 `：`；官方是半角 `{name}:`"),
                ),
                Malformed::SpaceBeforeColon => (
                    "LABEL_SPACE_BEFORE_COLON",
                    format!("`{name}` 与冒号之间不能有空格；官方是 `{name}:`"),
                ),
            };
            push(code, Severity::Error, i + 1, msg, evidence(line));
        }

        if in_fence[i] {
            if parse_label(line).is_some() {
                push(
                    "ROLE_IN_CODE_FENCE",
                    Severity::Warn,
                    i + 1,
                    "代码块里出现角色标签行；模型仍可能把它当轮次边界".to_string(),
                    evidence(line),
                );
            }
            if let Some(block) = current.as_mut() {
                block.body.push(i);
            } else {
                preamble.push(i);
            }
            continue;
        }

        if let Some((label, rest)) = parse_label(line) {
            if let Some(block) = current.take() {
                blocks.push(block);
            }
            current = Some(Block {
                label,
                line: i + 1,
                rest: rest.to_string(),
                body: Vec::new(),
            });
        } else if let Some(block) = current.as_mut() {
            block.body.push(i);
        } else {
            preamble.push(i);
        }
    }
    if let Some(block) = current.take() {
        blocks.push(block);
    }
    if continuation.is_some()
        && let Some(last) = blocks.last_mut()
    {
        // 续写标签前的空行是块分隔，不是上一块的正文空行。
        while last
            .body
            .last()
            .is_some_and(|&i| strip_cr(lines[i]).trim().is_empty())
        {
            last.body.pop();
        }
    }

    // ---- 统计与 kind ----
    for block in &blocks {
        match block.label {
            Label::User => stats.user_blocks += 1,
            Label::Assistant => stats.assistant_blocks += 1,
            Label::System => stats.system_blocks += 1,
            Label::Instruction => stats.instruction_blocks += 1,
            Label::Input => stats.input_blocks += 1,
            Label::Response => stats.response_blocks += 1,
        }
    }
    stats.metadata_lines = lines
        .iter()
        .filter(|raw| {
            let t = strip_cr(raw).trim_start();
            t.contains("[seg ") && t.contains('·')
        })
        .count();
    let mut run = 0usize;
    for raw in &lines {
        if strip_cr(raw).trim().is_empty() {
            run += 1;
            stats.max_blank_run = stats.max_blank_run.max(run);
        } else {
            run = 0;
        }
    }
    stats.function_call = blocks.iter().any(|b| {
        b.label == Label::System && block_text(&lines, b).trim_start().starts_with("Tools:")
    });

    let has_user = blocks.iter().any(|b| b.label == Label::User);
    let has_assistant = blocks.iter().any(|b| b.label == Label::Assistant);
    let has_instr = blocks.iter().any(|b| b.label == Label::Instruction);
    let has_input = blocks.iter().any(|b| b.label == Label::Input);
    let has_response = blocks.iter().any(|b| b.label == Label::Response);

    let kind = if chatml {
        Kind::Chatml
    } else if jsonl {
        Kind::Jsonl
    } else if markdown {
        Kind::Markdown
    } else if stats.function_call {
        Kind::SystemTools
    } else if has_instr && (has_input || has_response) {
        Kind::Instruction
    } else if has_instr && (has_user || has_assistant) {
        Kind::InstructionChat
    } else if continuation.is_some() {
        // 对话 + 末尾 `English:` 也是续写格式；不能因为前面有 User/Assistant
        // 就归成 plain_chat，否则 format 字段丢掉了真正的生成点类型。
        Kind::Continuation
    } else if has_user || has_assistant {
        if text.contains("材料：") || text.contains("问题：") || text.contains("{\"answer\"")
        {
            Kind::MaterialQa
        } else {
            Kind::PlainChat
        }
    } else {
        Kind::Unknown
    };

    // ---- 前导正文 / 无标签 ----
    let preamble_text: Vec<usize> = preamble
        .iter()
        .copied()
        .filter(|i| !strip_cr(lines[*i]).trim().is_empty())
        // 写法错误的角色标签已经由 malformed_label 报过，不再算「前导正文」。
        .filter(|i| malformed_label(strip_cr(lines[*i])).is_none())
        .collect();
    if !kind.foreign() && kind != Kind::Continuation {
        if blocks.is_empty() && continuation.is_none() {
            push(
                "NO_ROLE_LABEL",
                Severity::Error,
                1,
                "没有识别到任何 RWKV 角色标签；官方格式以 `User:` / `Assistant:` 或 \
                 `Instruction:` / `Input:` / `Response:` 开头（续写格式以 `English:` 等标签结尾）"
                    .to_string(),
                line_at(text, 0),
            );
        } else if let Some(&i) = preamble_text.first() {
            push(
                "PREAMBLE_TEXT",
                Severity::Error,
                i + 1,
                "第一个角色标签之前有正文；prompt 必须从 `System:` / `User:` / `Instruction:` 开始"
                    .to_string(),
                evidence(lines[i]),
            );
        }
    }

    // ---- 块间分隔 / 正文空行 ----
    //
    // 官方模板只用**一个空行**（`\n\n`）分隔块，且 System/User 输入还会被
    // `re.sub(r'\n{2,}', '\n', ...)` 折叠。因此 `\n\n` 是结构分隔符，不是正文换行：
    // 正文（含 Assistant 历史回答）里一律不允许空行，块间也不允许多于一个空行。
    if !kind.foreign() {
        // 开头空行：官方模板从标签行开始，clean_txt 会 strip。
        let leading = lines
            .iter()
            .take_while(|l| strip_cr(l).trim().is_empty())
            .count();
        if leading > 0 && !blocks.is_empty() {
            push(
                "LEADING_BLANK_LINES",
                Severity::Warn,
                leading,
                "prompt 开头有空行；官方模板从 `System:` / `User:` / `Instruction:` 开始"
                    .to_string(),
                evidence(lines[leading - 1]),
            );
        }

        for pair in blocks.windows(2) {
            let prev = &pair[0];
            let next = &pair[1];
            let trailing = trailing_blank(&lines, &prev.body);
            if trailing == 0 {
                push(
                    "MISSING_BLANK_SEPARATOR",
                    Severity::Error,
                    next.line,
                    format!(
                        "`{}` 与 `{}` 之间只隔了一个 `\\n`；官方模板固定用一个空行（`\\n\\n`）分隔",
                        label_name(prev.label),
                        label_name(next.label)
                    ),
                    format!(
                        "{} ⏎ {}",
                        evidence(lines[prev.line - 1]),
                        evidence(lines[next.line - 1])
                    ),
                );
            } else if trailing > 1 {
                push(
                    "EXTRA_BLANK_SEPARATOR",
                    Severity::Error,
                    next.line,
                    format!("块之间只能有一个空行（`\\n\\n`）；这里空了 {trailing} 行"),
                    evidence(lines[next.line - 1]),
                );
            } else {
                stats.blank_separators += 1;
            }

            // 空行只允许出现在块尾作为分隔；正文里出现就是结构错误（Assistant 也一样）。
            let content = &prev.body[..prev.body.len().saturating_sub(trailing)];
            if let Some(&i) = content
                .iter()
                .find(|i| strip_cr(lines[**i]).trim().is_empty())
            {
                push(
                    "BLANK_LINE_IN_CONTENT",
                    Severity::Error,
                    i + 1,
                    format!(
                        "`{}` 正文里有空行；`\\n\\n` 只允许作为块分隔，正文换行必须用单个 `\\n`\
                         （官方 clean_txt 会把 `\\n{{2,}}` 折成 `\\n`）",
                        label_name(prev.label)
                    ),
                    evidence(lines[i]),
                );
            }
        }

        // 续写标签与上一块之间同样是块分隔，必须恰好一个空行。
        if let Some((idx, head, _)) = continuation
            && let Some(last) = blocks.last()
        {
            let content_end = last
                .body
                .iter()
                .rposition(|&i| !strip_cr(lines[i]).trim().is_empty())
                .unwrap_or(last.line - 1);
            let blanks = idx.saturating_sub(content_end + 1);
            if blanks == 0 {
                push(
                    "MISSING_BLANK_SEPARATOR",
                    Severity::Error,
                    idx + 1,
                    format!(
                        "`{}` 与续写标签 `{head}:` 之间不能只隔一个 `\\n`；官方用一个空行分隔",
                        label_name(last.label)
                    ),
                    format!(
                        "{} ⏎ {}",
                        evidence(lines[content_end]),
                        evidence(lines[idx])
                    ),
                );
            } else if blanks > 1 {
                push(
                    "EXTRA_BLANK_SEPARATOR",
                    Severity::Error,
                    idx + 1,
                    format!("块与续写标签之间只能有一个空行；这里空了 {blanks} 行"),
                    evidence(lines[idx]),
                );
            } else {
                stats.blank_separators += 1;
            }
        }

        // 末尾块：文本以 `\n` 结尾时，最后那个空片段只是行终止符，不算空行。
        if let Some(last) = blocks.last() {
            let virtual_tail = usize::from(text.ends_with('\n'));
            let trailing = trailing_blank(&lines, &last.body);
            let blank_lines = trailing.saturating_sub(virtual_tail);
            if blank_lines > 0 {
                push(
                    "TRAILING_BLANK_LINES",
                    Severity::Error,
                    last.line,
                    format!(
                        "prompt 末尾有 {blank_lines} 个空行；官方模板停在 `Assistant:` / `Response:`，\
                         `\\n\\n` 只用于块之间"
                    ),
                    evidence(lines[last.line - 1]),
                );
            }
            let content = &last.body[..last.body.len().saturating_sub(trailing)];
            if let Some(&i) = content
                .iter()
                .find(|i| strip_cr(lines[**i]).trim().is_empty())
            {
                push(
                    "BLANK_LINE_IN_CONTENT",
                    Severity::Error,
                    i + 1,
                    format!(
                        "`{}` 正文里有空行；`\\n\\n` 只允许作为块分隔，正文换行必须用单个 `\\n`",
                        label_name(last.label)
                    ),
                    evidence(lines[i]),
                );
            }
        }
    }

    // ---- 逐块：空块 / 标签后的空白 ----
    if !kind.foreign() {
        for (idx, block) in blocks.iter().enumerate() {
            let last = idx + 1 == blocks.len();
            if block.label == Label::System {
                check_tools_label(&lines, block, &mut push);
            }
            let content = has_content(&lines, block);
            if !content {
                match block.label {
                    Label::User | Label::System | Label::Instruction => push(
                        "EMPTY_BLOCK",
                        Severity::Error,
                        block.line,
                        format!("`{}` 块是空的（标签后没有内容）", label_name(block.label)),
                        evidence(lines[block.line - 1]),
                    ),
                    Label::Input => push(
                        "EMPTY_INPUT",
                        Severity::Warn,
                        block.line,
                        "`Input:` 是空的；官方指令格式通常带输入".to_string(),
                        evidence(lines[block.line - 1]),
                    ),
                    Label::Assistant if !last => push(
                        "EMPTY_ASSISTANT_TURN",
                        Severity::Error,
                        block.line,
                        "中间有一轮空的 `Assistant:`；模型会把它当成空回答".to_string(),
                        evidence(lines[block.line - 1]),
                    ),
                    Label::Response if !last => push(
                        "EMPTY_RESPONSE_TURN",
                        Severity::Warn,
                        block.line,
                        "中间有一轮空的 `Response:`".to_string(),
                        evidence(lines[block.line - 1]),
                    ),
                    _ => {}
                }
            }

            if let Some(c) = block.rest.chars().next() {
                if c == '\u{3000}' || c == '\u{00a0}' {
                    push(
                        "LABEL_UNICODE_SPACE",
                        Severity::Error,
                        block.line,
                        format!(
                            "`{}:` 后面是全角/不换行空格；官方 G1 模板用半角空格",
                            label_name(block.label)
                        ),
                        evidence(lines[block.line - 1]),
                    );
                } else if c == '\t' {
                    push(
                        "LABEL_TAB",
                        Severity::Error,
                        block.line,
                        format!(
                            "`{}:` 后面是 Tab；官方 G1 模板用半角空格",
                            label_name(block.label)
                        ),
                        evidence(lines[block.line - 1]),
                    );
                } else if !c.is_whitespace() {
                    push(
                        "LABEL_NO_SPACE",
                        Severity::Error,
                        block.line,
                        format!(
                            "`{}:` 后没有空格就直接接内容；官方 G1 模板是 `{}: 内容`",
                            label_name(block.label),
                            label_name(block.label)
                        ),
                        evidence(lines[block.line - 1]),
                    );
                } else if block.rest.starts_with("  ") {
                    push(
                        "LABEL_EXTRA_SPACE",
                        Severity::Warn,
                        block.line,
                        format!(
                            "`{}:` 后多于一个空格；官方 G1 模板恰好一个半角空格",
                            label_name(block.label)
                        ),
                        evidence(lines[block.line - 1]),
                    );
                }
            }

            // 标签行只留下空白、正文从下一行开始：官方材料问答模板确实允许 `User:` 换行，
            // 但 `User: ⏎` 的尾随空格是纯噪声，还会改变标签后的 token 序列。
            if (!last || !matches!(block.label, Label::Assistant | Label::Response))
                && body_has_content(&lines, block)
                && !block.rest.is_empty()
                && block.rest.trim().is_empty()
            {
                push(
                    "LABEL_TRAILING_SPACE_BEFORE_NEWLINE",
                    Severity::Warn,
                    block.line,
                    format!(
                        "`{}:` 行尾有多余空白再换行；官方模板是 `{}:`（换行）或 `{}: 内容`",
                        label_name(block.label),
                        label_name(block.label),
                        label_name(block.label)
                    ),
                    format!("{}␠", evidence(lines[block.line - 1])),
                );
            }
        }
    }

    // ---- 轮次交替 ----
    if !kind.foreign() {
        let chat: Vec<&Block> = blocks
            .iter()
            .filter(|b| matches!(b.label, Label::User | Label::Assistant))
            .collect();
        if let Some(first) = chat.first()
            && first.label == Label::Assistant
        {
            push(
                "STARTS_WITH_ASSISTANT",
                Severity::Error,
                first.line,
                "prompt 不能以 `Assistant:` 开头（没有对应的用户问题）".to_string(),
                evidence(lines[first.line - 1]),
            );
        }
        for pair in chat.windows(2) {
            if pair[0].label == pair[1].label {
                let (code, msg) = match pair[0].label {
                    Label::User => ("TWO_USER_TURNS", "连续两个 User 块；缺一个 `Assistant:`"),
                    _ => (
                        "TWO_ASSISTANT_TURNS",
                        "连续两个 Assistant 块；缺一个对应的 `User:`",
                    ),
                };
                push(
                    code,
                    Severity::Error,
                    pair[1].line,
                    msg.to_string(),
                    evidence(lines[pair[1].line - 1]),
                );
            }
        }
    }

    // ---- System 头 ----
    if !kind.foreign() {
        let systems: Vec<&Block> = blocks.iter().filter(|b| b.label == Label::System).collect();
        if systems.len() > 1 {
            push(
                "MULTIPLE_SYSTEM",
                Severity::Error,
                systems[1].line,
                "只能有一个 `System:` 块".to_string(),
                evidence(lines[systems[1].line - 1]),
            );
        }
        if let Some(sys) = systems.first() {
            if blocks.first().is_some_and(|b| b.line != sys.line) {
                push(
                    "SYSTEM_NOT_FIRST",
                    Severity::Error,
                    sys.line,
                    "`System:` 必须在最前面".to_string(),
                    evidence(lines[sys.line - 1]),
                );
            }
            let body = block_text(&lines, sys);
            let head = body.trim_start();
            let first = head.lines().next().unwrap_or("").trim_end();
            if let Some(rest) = first.strip_prefix("Tools")
                && (rest.starts_with('：') || rest.starts_with('﹕') || rest.starts_with(" :"))
            {
                push(
                    "TOOLS_COLON",
                    Severity::Error,
                    sys.line,
                    "`Tools:` 必须用半角冒号且前面恰好一个空格；官方是 `System: Tools:`"
                        .to_string(),
                    evidence(lines[sys.line - 1]),
                );
            }
            if head.starts_with("Tools:") {
                push(
                    "SYSTEM_TOOLS",
                    Severity::Info,
                    sys.line,
                    "`System: Tools:`：官方仅 RWKV-7 G1x 的 function call 格式".to_string(),
                    evidence(lines[sys.line - 1]),
                );
                if sys.rest.trim().is_empty() {
                    push(
                        "TOOLS_NOT_INLINE",
                        Severity::Warn,
                        sys.line,
                        "`Tools:` 应在 `System:` 同一行（官方逐字 `System: Tools:`）".to_string(),
                        evidence(lines[sys.line - 1]),
                    );
                }
                if first.len() > "Tools:".len() {
                    push(
                        "TOOLS_NOT_ON_NEW_LINE",
                        Severity::Warn,
                        sys.line,
                        "`Tools:` 后应直接换行列出工具；官方模板是 `System: Tools:\\n- ...`"
                            .to_string(),
                        evidence(lines[sys.line - 1]),
                    );
                }
                if !body.contains("Return only a JSON function call.") {
                    push(
                        "TOOLS_MISSING_INSTRUCTION",
                        Severity::Warn,
                        sys.line,
                        "Tools 头缺少 `Return only a JSON function call.`".to_string(),
                        evidence(lines[sys.line - 1]),
                    );
                }
            } else if !first.starts_with("Tools") {
                push(
                    "SYSTEM_PERSONA",
                    Severity::Info,
                    sys.line,
                    "通用 `System:` 头是官方 G1 合法格式（`System: xxx\\n\\nUser: ...`）；\
                     注意 System 只能有一个，system 指令与记忆头必须合并进同一个 System 块"
                        .to_string(),
                    evidence(lines[sys.line - 1]),
                );
            }
            if let Some(pos) = blocks.iter().position(|b| b.line == sys.line)
                && let Some(next) = blocks.get(pos + 1)
                && !matches!(next.label, Label::User | Label::Instruction)
            {
                push(
                    "SYSTEM_THEN_ROLE",
                    Severity::Error,
                    next.line,
                    format!(
                        "`System:` 后面应接 `User:` 或 `Instruction:`，当前是 `{}:`",
                        label_name(next.label)
                    ),
                    evidence(lines[next.line - 1]),
                );
            }
        }
    }

    // ---- Instruction / Input / Response ----
    if !kind.foreign() {
        let instr_pos = blocks.iter().position(|b| b.label == Label::Instruction);
        let input_pos = blocks.iter().position(|b| b.label == Label::Input);
        let response_pos = blocks.iter().position(|b| b.label == Label::Response);

        if let Some(p) = input_pos
            && instr_pos.is_none()
        {
            push(
                "INPUT_WITHOUT_INSTRUCTION",
                Severity::Error,
                blocks[p].line,
                "`Input:` 前没有 `Instruction:`".to_string(),
                evidence(lines[blocks[p].line - 1]),
            );
        }
        if let Some(p) = response_pos
            && instr_pos.is_none()
        {
            push(
                "RESPONSE_WITHOUT_INSTRUCTION",
                Severity::Error,
                blocks[p].line,
                "`Response:` 前没有 `Instruction:`".to_string(),
                evidence(lines[blocks[p].line - 1]),
            );
        }
        if let (Some(i), Some(r)) = (instr_pos, response_pos) {
            if i > r {
                push(
                    "INSTRUCTION_ORDER",
                    Severity::Error,
                    blocks[i].line,
                    "`Instruction:` 必须在 `Response:` 之前".to_string(),
                    evidence(lines[blocks[i].line - 1]),
                );
            }
            if let Some(p) = input_pos
                && !(i < p && p < r)
            {
                push(
                    "INSTRUCTION_ORDER",
                    Severity::Error,
                    blocks[p].line,
                    "顺序必须是 `Instruction:` → `Input:` → `Response:`".to_string(),
                    evidence(lines[blocks[p].line - 1]),
                );
            }
            if input_pos.is_none() {
                push(
                    "INSTRUCTION_MISSING_INPUT",
                    Severity::Warn,
                    blocks[i].line,
                    "官方指令格式通常有 `Input:`；没有输入时可直接用 QA 格式".to_string(),
                    evidence(lines[blocks[i].line - 1]),
                );
            }
        }
        if (input_pos.is_some() || response_pos.is_some()) && (has_user || has_assistant) {
            push(
                "MIXED_INSTRUCTION_CHAT",
                Severity::Error,
                0,
                "`Instruction:/Input:/Response:` 与 `User:/Assistant:` 混在同一份 prompt；\
                 官方 few-shot 只允许 `Instruction:` 前缀 + User/Assistant 对，不允许混 Input/Response"
                    .to_string(),
                String::new(),
            );
        }
        if let Some(i) = instr_pos
            && response_pos.is_none()
            && !(has_user && has_assistant)
        {
            push(
                "INSTRUCTION_NO_RESPONSE",
                Severity::Warn,
                blocks[i].line,
                "有 `Instruction:` 但没有 `Response:`，也没有 User/Assistant 对；模型没有明确续写点"
                    .to_string(),
                evidence(lines[blocks[i].line - 1]),
            );
        }
    }

    // ---- 生成点 ----
    // 有续写标签时，最后一块（Assistant）只是历史回答，不能当生成点。
    let generation_block = if continuation.is_some() {
        None
    } else {
        blocks.last()
    };
    // 历史轮次里的 think 前缀同样校验（生成块在生成点分支里处理）。
    for b in &blocks {
        if b.label == Label::Assistant && generation_block.is_none_or(|last| last.line != b.line) {
            let _ = check_think_prefix(&lines, b, &mut push, false);
        }
    }

    let mut generation_point: Option<GenerationPoint> = None;
    if let Some(last) = generation_block {
        match last.label {
            Label::Assistant | Label::Response => {
                let seeded = has_content(&lines, last);
                let body_content = body_has_content(&lines, last);
                generation_point = Some(GenerationPoint {
                    line: last.line,
                    role: label_name(last.label).to_ascii_lowercase(),
                    seeded,
                });

                let think_like = last.label == Label::Assistant && {
                    let raw = raw_block(&lines, last);
                    let lead = raw.trim_start();
                    let low = lead.to_ascii_lowercase();
                    low.starts_with("<think") || low.starts_with("< think")
                };

                if think_like {
                    stats.think_prefix = check_think_prefix(&lines, last, &mut push, true);
                } else {
                    // 官方模板结尾是 `Assistant:`（无尾随空格）。本仓 compose 统一用
                    // `Assistant: `（一个空格）且实测可用——这里只记录，不判错。
                    if last.rest.is_empty() {
                        if body_content {
                            push(
                                "GEN_PREFIX_TRAILING_NEWLINE",
                                Severity::Warn,
                                last.line,
                                format!(
                                    "`{}:` 后直接换行再 seed 内容；官方模板把 seed 放在同一行",
                                    label_name(last.label)
                                ),
                                evidence(lines[last.line - 1]),
                            );
                        }
                    } else if last
                        .rest
                        .chars()
                        .any(|c| matches!(c, '\u{3000}' | '\u{00a0}'))
                    {
                        push(
                            "GEN_PREFIX_UNICODE_SPACE",
                            Severity::Warn,
                            last.line,
                            format!(
                                "`{}:` 后是全角/不换行空格；官方用一个半角空格",
                                label_name(last.label)
                            ),
                            evidence(lines[last.line - 1]),
                        );
                    } else if last.rest.chars().all(|c| c.is_whitespace()) {
                        let n = last.rest.chars().count();
                        if n > 1 {
                            push(
                                "GEN_PREFIX_EXTRA_SPACE",
                                Severity::Warn,
                                last.line,
                                format!(
                                    "`{}:` 后有 {n} 个空白；官方结尾不带尾随空格，\
                                     本仓统一用一个半角空格",
                                    label_name(last.label)
                                ),
                                evidence(lines[last.line - 1]),
                            );
                        } else if body_content {
                            push(
                                "GEN_PREFIX_TRAILING_NEWLINE",
                                Severity::Warn,
                                last.line,
                                format!(
                                    "`{}:` 行尾有空格后换行再 seed 内容；官方是 `{}:` 或 `{}: 内容`",
                                    label_name(last.label),
                                    label_name(last.label),
                                    label_name(last.label)
                                ),
                                evidence(lines[last.line - 1]),
                            );
                        } else {
                            push(
                                "GEN_PREFIX_TRAILING_SPACE",
                                Severity::Info,
                                last.line,
                                if last.label == Label::Assistant {
                                    "官方 G1x 模板以 `Assistant:` 结尾（无尾随空格）；\
                                     本仓 compose 统一用 `Assistant: `（一个空格），实测可用——\
                                     保持全链路一致即可"
                                        .to_string()
                                } else {
                                    "官方示例以 `Response:` 结尾（无尾随空格）；这里多了一个尾随空格"
                                        .to_string()
                                },
                                format!("{}␠", evidence(lines[last.line - 1])),
                            );
                        }
                    }
                    if seeded {
                        push(
                            "SEEDED_GENERATION_POINT",
                            Severity::Info,
                            last.line,
                            "生成点已 seed 内容（材料问答 `{\"answer\": ` / 代码块 / 翻译前缀等）"
                                .to_string(),
                            evidence(lines[last.line - 1]),
                        );
                    }
                }
            }
            Label::User => push(
                "ENDS_ON_USER",
                Severity::Error,
                last.line,
                "prompt 停在 `User:` 块；模型会续写用户发言而不是回答。末尾必须是 `Assistant: `"
                    .to_string(),
                evidence(lines[last.line - 1]),
            ),
            _ => push(
                "ENDS_ON_NON_ASSISTANT",
                Severity::Error,
                last.line,
                format!(
                    "prompt 停在 `{}:`；官方续写点是 `Assistant:`（QA）或 `Response:`（指令）",
                    label_name(last.label)
                ),
                evidence(lines[last.line - 1]),
            ),
        }
    }

    // ---- 续写格式的生成点 ----
    if !kind.foreign()
        && let Some((idx, head, tail)) = continuation
    {
        generation_point = Some(GenerationPoint {
            line: idx + 1,
            role: head.to_ascii_lowercase(),
            seeded: !tail.trim().is_empty(),
        });
        push(
            "CONTINUATION_LABEL",
            Severity::Info,
            idx + 1,
            format!("续写格式：以 `{head}:` 结尾，符合官方示例"),
            evidence(lines[idx]),
        );
    }

    // ---- think 长度控制（半角括号 / 前置一个半角空格 / 位于 User 末尾）----
    if !kind.foreign() {
        for user in blocks.iter().filter(|b| b.label == Label::User) {
            check_think_hints(&lines, user, &mut push);
        }
    }

    // ---- 本项目的记忆元数据 ----
    if let Some((i, _)) = lines.iter().enumerate().find(|(_, raw)| {
        let t = strip_cr(raw).trim_start();
        t.contains("[seg ") && t.contains('·')
    }) {
        push(
            "MEM_META",
            Severity::Warn,
            i + 1,
            "记忆摘录带 `[seg N · role]` 元数据；官方格式没有它，实测它与 `System:` 指令同块时\
             会把检索答成「I don't have access」（同内容去元数据后答对）。建议只把摘录文本放进 prompt"
                .to_string(),
            evidence(lines[i]),
        );
    }

    finish(kind, findings, stats, generation_point)
}

fn finish(
    kind: Kind,
    mut findings: Vec<Finding>,
    stats: FormatStats,
    generation_point: Option<GenerationPoint>,
) -> FormatReport {
    findings.sort_by(|a, b| {
        a.line
            .cmp(&b.line)
            .then_with(|| a.severity.rank().cmp(&b.severity.rank()))
            .then_with(|| a.code.cmp(b.code))
    });
    findings.dedup_by(|a, b| a.code == b.code && a.line == b.line);
    let error_count = findings
        .iter()
        .filter(|f| f.severity == Severity::Error)
        .count();
    let warning_count = findings
        .iter()
        .filter(|f| f.severity == Severity::Warn)
        .count();
    let info_count = findings
        .iter()
        .filter(|f| f.severity == Severity::Info)
        .count();
    FormatReport {
        profile: "g1",
        format: kind.as_str(),
        ok: error_count == 0,
        error_count,
        warning_count,
        info_count,
        generation_point,
        stats,
        findings,
    }
}
