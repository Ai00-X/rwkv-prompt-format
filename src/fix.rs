//! 自动修复：**确定性、幂等、不做语义改写**。
//!
//! 管线是「解析 → 规范块 → 变换 → 渲染 G1 官方 plain 格式」：
//!
//! - 安全修复（`safe`）：BOM / CRLF / 标签写法（大小写、缩进、全角冒号、冒号前空格）/
//!   标签后的空白 / 块间空行 / 正文空行 / think 前缀与 `(think)` 间距 / `Tools:` 写法；
//! - 结构修复（`structural`）：合并多个 `System:` 并移到最前、合并连续同角色块、
//!   转换 ChatML / Llama 3 / JSONL / Markdown / `Human:` `AI:` / `Question:` `Answer:`、
//!   重排 `Instruction:` → `Input:` → `Response:`、补末尾生成点。
//!
//! 修复结果会用同一个检测器重新检查，`applied` 只列**确实减少**的 finding code，
//! 所以报告不会自说自话。已知边界：不认识的内容（如 Llama 2 `[INST]`、任意
//! `(thinking)`）不会被猜着改，会留在 `after.findings` 里。

use serde::Serialize;
use serde_json::Value;

use crate::check::{KNOWN, Label, check, label_name, parse_label, strip_cr};
use crate::types::{FormatReport, Severity};

/// 安全修复：只动语法与空白，不增删块。
pub const FIX_SAFE: u32 = 0x1;
/// 结构修复：可合并/移动/转换/补生成点。
pub const FIX_STRUCTURAL: u32 = 0x2;
/// 去掉 `[seg 12 · user]` 这类记忆元数据前缀（默认不做，因为属于内容编辑）。
pub const FIX_STRIP_METADATA: u32 = 0x4;

/// 修复范围。
#[derive(Debug, Clone, Serialize)]
pub struct FixOptions {
    pub safe: bool,
    pub structural: bool,
    pub strip_metadata: bool,
}

impl Default for FixOptions {
    fn default() -> Self {
        Self {
            safe: true,
            structural: true,
            strip_metadata: false,
        }
    }
}

impl FixOptions {
    pub fn safe_only() -> Self {
        Self {
            safe: true,
            structural: false,
            strip_metadata: false,
        }
    }

    pub fn structural_only() -> Self {
        Self {
            safe: false,
            structural: true,
            strip_metadata: false,
        }
    }

    pub fn from_flags(flags: u32) -> Self {
        Self {
            safe: flags & FIX_SAFE != 0,
            structural: flags & FIX_STRUCTURAL != 0,
            strip_metadata: flags & FIX_STRIP_METADATA != 0,
        }
    }

    pub fn flags(&self) -> u32 {
        (u32::from(self.safe) * FIX_SAFE)
            | (u32::from(self.structural) * FIX_STRUCTURAL)
            | (u32::from(self.strip_metadata) * FIX_STRIP_METADATA)
    }
}

/// 一类被修复的问题（按 finding code 归并）。
#[derive(Debug, Clone, Serialize)]
pub struct AppliedFix {
    pub code: String,
    pub count: usize,
    pub message: String,
}

/// 一次修复的完整产物。`text` 是修复后的 prompt，`before/after` 是同一检测器的两次报告。
#[derive(Debug, Clone, Serialize)]
pub struct FixResult {
    pub changed: bool,
    pub options: FixOptions,
    pub applied: Vec<AppliedFix>,
    pub notes: Vec<String>,
    pub before: FormatReport,
    pub after: FormatReport,
    pub text: String,
}

/// 修复一段 prompt。纯文本、不加载模型、不读写磁盘。
pub fn fix(text: &str, options: &FixOptions) -> FixResult {
    let before = check(text);
    let mut notes: Vec<String> = Vec::new();

    let fixed = if !options.safe && !options.structural {
        text.to_string()
    } else {
        rewrite(text, options, &mut notes)
    };

    let after = check(&fixed);
    let applied = diff_applied(&before, &after, fixed != text);
    FixResult {
        changed: fixed != text,
        options: options.clone(),
        applied,
        notes,
        before,
        after,
        text: fixed,
    }
}

// ---------------------------------------------------------------------------
// 主流程
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Msg {
    label: Label,
    content: String,
    /// 原文内容是否从标签同一行开始。官方材料模板是 `User:` 换行再写材料，
    /// 不能一律并到标签行；`Tools:` / `<think` 例外，必须内联。
    inline: bool,
}

#[derive(Debug, Clone, Default)]
struct Parsed {
    msgs: Vec<Msg>,
    /// 第一个角色标签之前的正文（原样保留，结构模式会并入 System）。
    preamble: String,
}

fn rewrite(text: &str, options: &FixOptions, notes: &mut Vec<String>) -> String {
    // BOM / CRLF 是字节级问题，任何修复模式都先处理。
    let work = text
        .strip_prefix('\u{feff}')
        .unwrap_or(text)
        .replace("\r\n", "\n")
        .replace('\r', "\n");

    if work.trim().is_empty() {
        notes.push("输入为空，未做任何修改".to_string());
        return work;
    }

    let mut parsed = if options.structural {
        convert_foreign(&work, notes).unwrap_or_else(|| parse_blocks(&work, true, notes))
    } else {
        parse_blocks(&work, false, notes)
    };

    let last_idx = parsed.msgs.len().saturating_sub(1);
    for idx in 0..parsed.msgs.len() {
        normalize_msg(&mut parsed.msgs[idx], idx == last_idx, notes);
    }

    if options.strip_metadata {
        strip_metadata(&mut parsed.msgs);
    }

    if options.structural {
        if !parsed.preamble.trim().is_empty() {
            let content = clean_text(&parsed.preamble);
            if !content.is_empty() {
                // 完全无标签的裸文本更像一个问题 → User；已有标签时，前导正文是上下文 → System。
                let label = if parsed.msgs.is_empty() {
                    notes.push("没有角色标签：整段文本已包装为 `User:` 并补生成点".to_string());
                    Label::User
                } else {
                    notes.push("第一个角色标签之前的正文已并入 `System:`".to_string());
                    Label::System
                };
                parsed.msgs.insert(
                    0,
                    Msg {
                        label,
                        content,
                        inline: true,
                    },
                );
            }
        }
        transform(&mut parsed.msgs, notes);
        render(&parsed.msgs, "")
    } else {
        render(&parsed.msgs, &parsed.preamble)
    }
}

/// `System: Tools:` 与 `Assistant: <think` 必须和标签同一行（官方逐字）。
fn must_inline(msg: &Msg) -> bool {
    match msg.label {
        Label::System => msg.content.trim_start().starts_with("Tools:"),
        Label::Assistant => {
            let low = msg.content.trim_start().to_ascii_lowercase();
            low.starts_with("<think") || low.starts_with("< think")
        }
        _ => false,
    }
}

fn render(msgs: &[Msg], preamble: &str) -> String {
    let mut blocks: Vec<String> = Vec::new();
    if !preamble.trim().is_empty() {
        let p = clean_text(preamble);
        if !p.is_empty() {
            blocks.push(p);
        }
    }
    for msg in msgs {
        let name = label_name(msg.label);
        if msg.content.is_empty() {
            blocks.push(format!("{name}:"));
        } else if msg.inline || must_inline(msg) {
            blocks.push(format!("{name}: {}", msg.content));
        } else {
            blocks.push(format!(
                "{name}:
{}",
                msg.content
            ));
        }
    }
    if blocks.is_empty() {
        return String::new();
    }
    let mut out = blocks.join("\n\n");
    out.push('\n');
    out
}

/// 正文清洗：按行去掉空行（官方 `clean_txt` 的 `\n{2,}` → `\n`）。
/// **不 trim 行尾空白**，避免吃掉 `Assistant: {"answer": ` 的 seed 空格。
fn clean_text(text: &str) -> String {
    let mut lines: Vec<String> = text
        .split('\n')
        .filter(|line| !strip_cr(line).trim().is_empty())
        .map(|line| strip_cr(line).to_string())
        .collect();
    while lines.first().is_some_and(|line| line.trim().is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    if let Some(first) = lines.first_mut() {
        *first = first.trim_start().to_string();
    }
    lines.join("\n")
}

// ---------------------------------------------------------------------------
// 解析
// ---------------------------------------------------------------------------

fn parse_blocks(text: &str, structural: bool, _notes: &mut Vec<String>) -> Parsed {
    let mut out = Parsed::default();
    let mut current: Option<Msg> = None;
    let mut preamble: Vec<String> = Vec::new();
    let mut in_fence = false;

    for raw in text.split('\n') {
        let line = strip_cr(raw);
        let fence_line = line.trim_start().starts_with("```");
        if in_fence {
            if fence_line {
                in_fence = false;
            }
            push_line(&mut current, &mut preamble, line);
            continue;
        }
        if fence_line {
            in_fence = true;
            push_line(&mut current, &mut preamble, line);
            continue;
        }
        if let Some((label, rest, _canonical)) = split_label_any(line, structural) {
            let opens_fence = rest.trim_start().starts_with("```");
            out.msgs.extend(current.take());
            current = Some(Msg {
                label,
                content: rest.trim_start().to_string(),
                inline: !rest.trim_start().is_empty(),
            });
            if opens_fence {
                in_fence = true;
            }
            continue;
        }
        push_line(&mut current, &mut preamble, line);
    }
    out.msgs.extend(current.take());
    out.preamble = preamble.join("\n");
    out
}

fn push_line(current: &mut Option<Msg>, preamble: &mut Vec<String>, line: &str) {
    match current {
        Some(msg) => {
            if !msg.content.is_empty() {
                msg.content.push('\n');
            }
            msg.content.push_str(line);
        }
        None => preamble.push(line.to_string()),
    }
}

/// 解析**任意写法**的标签：官方标签（含缩进/大小写/全角冒号/冒号前空格）+
/// 可选的历史别名（`Human:` / `AI:` / `Question:` / `Answer:`）。
/// 返回 `(label, 冒号之后的内容, 是否官方逐字写法)`。
fn split_label_any(line: &str, structural: bool) -> Option<(Label, &str, bool)> {
    if let Some((label, rest)) = parse_label(line) {
        return Some((label, rest, true));
    }
    let trimmed = line.trim_start();
    if trimmed.is_empty() {
        return None;
    }
    for (name, label) in KNOWN {
        if let Some(rest) = loose_label(trimmed, name) {
            return Some((label, rest, false));
        }
    }
    if structural {
        const ALIASES: [(&str, Label); 4] = [
            ("Human", Label::User),
            ("AI", Label::Assistant),
            ("Question", Label::User),
            ("Answer", Label::Assistant),
        ];
        for (name, label) in ALIASES {
            if let Some(rest) = loose_label(trimmed, name) {
                return Some((label, rest, false));
            }
        }
    }
    None
}

fn loose_label<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let head = line.get(..name.len())?;
    if !head.eq_ignore_ascii_case(name) {
        return None;
    }
    let after = &line[name.len()..];
    let after = after.trim_start_matches([' ', '\t']);
    for colon in [':', '：', '﹕', '︓'] {
        if let Some(rest) = after.strip_prefix(colon) {
            return Some(rest);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// 单块规范化（安全修复）
// ---------------------------------------------------------------------------

fn normalize_msg(msg: &mut Msg, is_last: bool, notes: &mut Vec<String>) {
    let had_fence_blank =
        msg.content.split('\n').any(|line| line.trim().is_empty()) && msg.content.contains("```");

    let mut lines: Vec<String> = msg
        .content
        .split('\n')
        .filter(|line| !line.trim().is_empty())
        .map(|line| strip_cr(line).to_string())
        .collect();
    // 只有内容原本就在标签行时才去缩进；`User:` 换行后的材料排版要保留。
    if msg.inline
        && let Some(first) = lines.first_mut()
    {
        *first = first.trim_start().to_string();
    }
    let mut content = lines.join("\n");

    match msg.label {
        Label::System => content = normalize_tools(&content),
        Label::Assistant => content = normalize_think_prefix(&content),
        Label::User => content = normalize_think_hints(&content),
        _ => {}
    }

    if had_fence_blank && content.contains("```") {
        notes.push("代码围栏内的空行已按官方 clean_txt 折叠（会改变代码排版，请复核）".to_string());
    }
    // 末尾块的 seed 空格要保留；非末尾块的裸标签空白已由 `content.is_empty()` 统一。
    if !is_last {
        content = content.trim_end().to_string();
    }
    msg.content = content;
}

/// `System:` 块首行的 `Tools:` 写法归一。只认首行，避免误改正文。
fn normalize_tools(content: &str) -> String {
    let mut lines: Vec<String> = content.split('\n').map(str::to_string).collect();
    let Some(first) = lines.first_mut() else {
        return content.to_string();
    };
    let t = first.trim_start();
    let Some(head) = t.get(..5) else {
        return content.to_string();
    };
    if !head.eq_ignore_ascii_case("Tools") {
        return content.to_string();
    }
    let after = &t[5..];
    let after_space = after.trim_start_matches([' ', '\t']);
    let rest = [':', '：', '﹕', '︓']
        .iter()
        .find_map(|colon| after_space.strip_prefix(*colon));
    let Some(rest) = rest else {
        return content.to_string();
    };
    let rest = rest.trim_start();
    *first = if rest.is_empty() {
        "Tools:".to_string()
    } else {
        format!("Tools: {rest}")
    };
    lines.join("\n")
}

/// 官方三种 think 前缀：`<think`、`<think></think`、`<think>\n</think`。
/// 只在**整体形态可判定**时才改写，历史轮次里带推理正文的 `<think>...` 不动。
fn normalize_think_prefix(content: &str) -> String {
    let t = content.trim();
    if t.is_empty() {
        return content.to_string();
    }
    let mut sig = String::new();
    for ch in t.chars() {
        if ch.is_whitespace() || ch == '>' {
            continue;
        }
        sig.push(ch.to_ascii_lowercase());
    }
    let canonical = match sig.as_str() {
        "<think" => Some("<think"),
        "<think</think" => {
            if t.contains('\n') {
                Some("<think>\n</think")
            } else {
                Some("<think></think")
            }
        }
        _ => None,
    };
    match canonical {
        Some(c) => c.to_string(),
        None => content.to_string(),
    }
}

/// 把 `(think)` / `(think a bit)` / `(think a lot)` 归一为半角、单个前置空格、
/// 位于 User 文本末尾。代码围栏里的内容不动。
fn normalize_think_hints(content: &str) -> String {
    let mut out_lines: Vec<String> = Vec::new();
    let mut hint: Option<&'static str> = None;
    let mut in_fence = false;
    for line in content.split('\n') {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            out_lines.push(line.to_string());
            continue;
        }
        if in_fence {
            out_lines.push(line.to_string());
            continue;
        }
        let (cleaned, found) = strip_hints_from_line(line);
        if found.is_some() {
            hint = found;
        }
        out_lines.push(cleaned);
    }
    let mut out = out_lines.join("\n").trim_end().to_string();
    if let Some(hint) = hint {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(hint);
    }
    out
}

fn strip_hints_from_line(line: &str) -> (String, Option<&'static str>) {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::new();
    let mut hint = None;
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c == '(' || c == '（' {
            let mut close = None;
            let mut j = i + 1;
            while j < chars.len() && j <= i + 48 {
                if chars[j] == ')' || chars[j] == '）' {
                    close = Some(j);
                    break;
                }
                j += 1;
            }
            if let Some(j) = close {
                let inner: String = chars[i + 1..j].iter().collect();
                if let Some(canonical) = canonical_hint(&inner) {
                    hint = Some(canonical);
                    while out.ends_with([' ', '\t']) {
                        out.pop();
                    }
                    i = j + 1;
                    if let Some(&next) = chars.get(i)
                        && !next.is_whitespace()
                        && !out.is_empty()
                        && !out.ends_with(char::is_whitespace)
                    {
                        out.push(' ');
                    }
                    continue;
                }
            }
        }
        out.push(c);
        i += 1;
    }
    (out, hint)
}

fn canonical_hint(inner: &str) -> Option<&'static str> {
    let collapsed = inner.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.eq_ignore_ascii_case("think") {
        Some("(think)")
    } else if collapsed.eq_ignore_ascii_case("think a bit") {
        Some("(think a bit)")
    } else if collapsed.eq_ignore_ascii_case("think a lot") {
        Some("(think a lot)")
    } else {
        None
    }
}

fn strip_metadata(msgs: &mut [Msg]) {
    for msg in msgs {
        let lines: Vec<String> = msg
            .content
            .split('\n')
            .filter_map(|line| {
                let t = line.trim_start();
                if let Some(rest) = strip_seg_prefix(t) {
                    let rest = rest.trim_start();
                    if rest.is_empty() {
                        None
                    } else {
                        Some(rest.to_string())
                    }
                } else {
                    Some(line.to_string())
                }
            })
            .collect();
        msg.content = lines.join("\n");
    }
}

fn strip_seg_prefix(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("[seg ")?;
    let close = rest.find(']')?;
    let meta = &rest[..close];
    meta.contains('·').then_some(&rest[close + 1..])
}

// ---------------------------------------------------------------------------
// 结构变换
// ---------------------------------------------------------------------------

fn transform(msgs: &mut Vec<Msg>, notes: &mut Vec<String>) {
    drop_empty(msgs);
    merge_system(msgs, notes);
    merge_same_role(msgs);
    reorder_instruction(msgs);
    ensure_generation_point(msgs);
    audit_residual(msgs, notes);
}

fn drop_empty(msgs: &mut Vec<Msg>) {
    let mut i = 0usize;
    while i < msgs.len() {
        let empty = msgs[i].content.trim().is_empty();
        let last = i + 1 == msgs.len();
        let gen_point = matches!(msgs[i].label, Label::Assistant | Label::Response);
        if empty && !(last && gen_point) {
            msgs.remove(i);
        } else {
            i += 1;
        }
    }
}

fn merge_system(msgs: &mut Vec<Msg>, notes: &mut Vec<String>) {
    let mut positions: Vec<usize> = msgs
        .iter()
        .enumerate()
        .filter(|(_, m)| m.label == Label::System)
        .map(|(i, _)| i)
        .collect();
    if positions.is_empty() {
        return;
    }
    if positions.len() > 1 {
        // `Tools:` 是 function call 的固定头，必须放在合并后 System 的第一行。
        let has_tools = positions
            .iter()
            .any(|&i| msgs[i].content.trim_start().starts_with("Tools:"));
        positions.sort_by_key(|&i| !msgs[i].content.trim_start().starts_with("Tools:"));
        if has_tools {
            notes.push(
                "多个 `System:` 中含 `Tools:`：已把 Tools 块放最前，请复核 function call 指令"
                    .to_string(),
            );
        }
        let mut merged = String::new();
        for &i in &positions {
            let content = msgs[i].content.trim_matches('\n');
            if content.is_empty() {
                continue;
            }
            if !merged.is_empty() {
                merged.push('\n');
            }
            merged.push_str(content);
        }
        // 删除必须按**原始下标降序**；合并顺序（Tools 优先）不能拿来删。
        let mut removal = positions.clone();
        removal.sort_unstable_by(|a, b| b.cmp(a));
        for i in removal {
            msgs.remove(i);
        }
        if !merged.is_empty() {
            msgs.insert(
                0,
                Msg {
                    label: Label::System,
                    content: merged,
                    inline: true,
                },
            );
        }
    } else if positions[0] != 0 {
        let sys = msgs.remove(positions[0]);
        msgs.insert(0, sys);
    }
}

fn merge_same_role(msgs: &mut Vec<Msg>) {
    let mut i = 1usize;
    while i < msgs.len() {
        if msgs[i].label == msgs[i - 1].label {
            let content = std::mem::take(&mut msgs[i].content);
            let prev = &mut msgs[i - 1];
            if !prev.content.is_empty() && !content.is_empty() {
                prev.content.push('\n');
            }
            prev.content.push_str(&content);
            msgs.remove(i);
        } else {
            i += 1;
        }
    }
}

fn reorder_instruction(msgs: &mut Vec<Msg>) {
    let has_chat = msgs
        .iter()
        .any(|m| matches!(m.label, Label::User | Label::Assistant));
    let has_instruction = msgs.iter().any(|m| m.label == Label::Instruction);
    if has_chat || !has_instruction {
        return;
    }
    let all_known = msgs.iter().all(|m| {
        matches!(
            m.label,
            Label::System | Label::Instruction | Label::Input | Label::Response
        )
    });
    if !all_known {
        return;
    }
    let mut ordered: Vec<Msg> = Vec::with_capacity(msgs.len());
    for label in [
        Label::System,
        Label::Instruction,
        Label::Input,
        Label::Response,
    ] {
        ordered.extend(msgs.iter().filter(|m| m.label == label).cloned());
    }
    *msgs = ordered;
}

fn ensure_generation_point(msgs: &mut Vec<Msg>) {
    if msgs.is_empty() {
        return;
    }
    if matches!(
        msgs.last().map(|m| m.label),
        Some(Label::Assistant | Label::Response)
    ) {
        return;
    }
    let instruction = msgs.iter().any(|m| m.label == Label::Instruction);
    let chat = msgs
        .iter()
        .any(|m| matches!(m.label, Label::User | Label::Assistant));
    msgs.push(Msg {
        label: if instruction && !chat {
            Label::Response
        } else {
            Label::Assistant
        },
        content: String::new(),
        inline: false,
    });
}

fn audit_residual(msgs: &[Msg], notes: &mut Vec<String>) {
    if msgs.first().is_some_and(|m| m.label == Label::Assistant) {
        notes.push("开头是 `Assistant:`（无对应 `User:`）：删除会丢内容，未自动修复".to_string());
    }
    let has_chat = msgs
        .iter()
        .any(|m| matches!(m.label, Label::User | Label::Assistant));
    let has_instruction = msgs
        .iter()
        .any(|m| matches!(m.label, Label::Instruction | Label::Input | Label::Response));
    if has_chat && has_instruction {
        notes.push(
            "`Instruction:/Input:/Response:` 与 `User:/Assistant:` 混用：属于内容结构问题，未自动修复"
                .to_string(),
        );
    }
    if msgs
        .iter()
        .any(|m| m.label == Label::Input || m.label == Label::Response)
        && !msgs.iter().any(|m| m.label == Label::Instruction)
    {
        notes.push("`Input:/Response:` 缺少 `Instruction:`：未自动猜测指令内容".to_string());
    }
}

// ---------------------------------------------------------------------------
// 外部模板转换
// ---------------------------------------------------------------------------

fn convert_foreign(text: &str, notes: &mut Vec<String>) -> Option<Parsed> {
    if text.contains("<|im_start|>") {
        let parsed = convert_chatml(text, notes);
        notes.push("已把 ChatML 模板转换为 G1 plain".to_string());
        return Some(parsed);
    }
    if text.contains("<|start_header_id|>") {
        let parsed = convert_llama3(text, notes);
        notes.push("已把 Llama 3 模板转换为 G1 plain".to_string());
        return Some(parsed);
    }
    if let Some(parsed) = convert_jsonl(text, notes) {
        notes.push("已把 JSONL / OpenAI messages 转换为 G1 plain".to_string());
        return Some(parsed);
    }
    if has_markdown_role(text) {
        let parsed = convert_markdown(text);
        notes.push("已把 Markdown 角色头转换为 G1 plain".to_string());
        return Some(parsed);
    }
    None
}

fn convert_chatml(text: &str, notes: &mut Vec<String>) -> Parsed {
    const START: &str = "<|im_start|>";
    const END: &str = "<|im_end|>";
    let mut out = Parsed::default();
    let mut cursor = text;
    if let Some(idx) = text.find(START) {
        out.preamble = strip_protocol_markers(&text[..idx]);
    }
    while let Some(start) = cursor.find(START) {
        cursor = &cursor[start + START.len()..];
        let (role, body) = cursor.split_once('\n').unwrap_or((cursor, ""));
        let (content, after) = match body.find(END) {
            Some(idx) => (&body[..idx], &body[idx + END.len()..]),
            None => (body, ""),
        };
        if let Some(label) = chat_label(role.trim()) {
            out.msgs.push(Msg {
                label,
                content: content.trim_matches('\n').to_string(),
                inline: true,
            });
        } else {
            notes.push(format!(
                "ChatML 角色 `{}` 在 G1 没有对应标签，已跳过该条",
                role.trim()
            ));
        }
        cursor = after;
    }
    if !cursor.trim().is_empty() {
        out.preamble.push('\n');
        out.preamble.push_str(&strip_protocol_markers(cursor));
    }
    out
}

fn convert_llama3(text: &str, notes: &mut Vec<String>) -> Parsed {
    const START: &str = "<|start_header_id|>";
    const HEAD: &str = "<|end_header_id|>";
    const END: &str = "<|eot_id|>";
    let mut out = Parsed::default();
    let mut cursor = text;
    if let Some(idx) = text.find(START) {
        out.preamble = strip_protocol_markers(&text[..idx]);
    }
    while let Some(start) = cursor.find(START) {
        cursor = &cursor[start + START.len()..];
        let Some((role, body)) = cursor.split_once(HEAD) else {
            break;
        };
        let body = body.trim_start_matches('\n');
        let (content, after) = match body.find(END) {
            Some(idx) => (&body[..idx], &body[idx + END.len()..]),
            None => (body, ""),
        };
        if let Some(label) = chat_label(role.trim()) {
            out.msgs.push(Msg {
                label,
                content: strip_protocol_markers(content)
                    .trim_matches('\n')
                    .to_string(),
                inline: true,
            });
        } else {
            notes.push(format!(
                "Llama 3 角色 `{}` 在 G1 没有对应标签，已跳过该条",
                role.trim()
            ));
        }
        cursor = after;
    }
    if !cursor.trim().is_empty() {
        out.preamble.push('\n');
        out.preamble.push_str(&strip_protocol_markers(cursor));
    }
    out
}

fn chat_label(role: &str) -> Option<Label> {
    match role.to_ascii_lowercase().as_str() {
        "system" => Some(Label::System),
        "user" => Some(Label::User),
        "assistant" => Some(Label::Assistant),
        _ => None,
    }
}

fn strip_protocol_markers(text: &str) -> String {
    text.replace("<|begin_of_text|>", "")
        .replace("<|end_of_text|>", "")
        .replace("<|endoftext|>", "")
        .replace("<|eot_id|>", "")
        .trim_matches('\n')
        .to_string()
}

fn convert_jsonl(text: &str, notes: &mut Vec<String>) -> Option<Parsed> {
    let trimmed = text.trim();
    if (trimmed.starts_with('[') || trimmed.starts_with('{'))
        && let Ok(value) = serde_json::from_str::<Value>(trimmed)
    {
        let array = value
            .as_array()
            .cloned()
            .or_else(|| value.get("messages")?.as_array().cloned());
        if let Some(array) = array {
            let mut out = Parsed::default();
            for item in &array {
                let role = item.get("role")?.as_str()?;
                let Some(label) = chat_label(role) else {
                    notes.push(format!("JSON 角色 `{role}` 在 G1 没有对应标签，未转换"));
                    return None;
                };
                let content = match item.get("content")? {
                    Value::String(s) => s.clone(),
                    Value::Array(parts) => parts
                        .iter()
                        .filter_map(|p| p.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    _ => return None,
                };
                out.msgs.push(Msg {
                    label,
                    content,
                    inline: true,
                });
            }
            return (!out.msgs.is_empty()).then_some(out);
        }
    }

    let mut out = Parsed::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(line).ok()?;
        let role = value.get("role")?.as_str()?;
        let Some(label) = chat_label(role) else {
            notes.push(format!("JSONL 角色 `{role}` 在 G1 没有对应标签，未转换"));
            return None;
        };
        let content = value.get("content")?.as_str()?.to_string();
        out.msgs.push(Msg {
            label,
            content,
            inline: true,
        });
    }
    (!out.msgs.is_empty()).then_some(out)
}

fn has_markdown_role(text: &str) -> bool {
    let mut in_fence = false;
    for raw in text.split('\n') {
        let line = strip_cr(raw);
        let fence_line = line.trim_start().starts_with("```");
        if in_fence {
            if fence_line {
                in_fence = false;
            }
            continue;
        }
        if fence_line {
            in_fence = true;
            continue;
        }
        if let Some((_, rest, _)) = split_label_any(line, true) {
            if rest.trim_start().starts_with("```") {
                in_fence = true;
            }
            continue;
        }
        if markdown_header(line).is_some() {
            return true;
        }
    }
    false
}

fn convert_markdown(text: &str) -> Parsed {
    let mut out = Parsed::default();
    let mut current: Option<Msg> = None;
    let mut preamble: Vec<String> = Vec::new();
    let mut in_fence = false;

    for raw in text.split('\n') {
        let line = strip_cr(raw);
        let fence_line = line.trim_start().starts_with("```");
        if in_fence {
            if fence_line {
                in_fence = false;
            }
            push_line(&mut current, &mut preamble, line);
            continue;
        }
        if fence_line {
            in_fence = true;
            push_line(&mut current, &mut preamble, line);
            continue;
        }
        if let Some((label, rest)) = markdown_header(line) {
            out.msgs.extend(current.take());
            current = Some(Msg {
                label,
                content: rest.trim_start().to_string(),
                inline: !rest.trim_start().is_empty(),
            });
            continue;
        }
        if let Some((label, rest, _)) = split_label_any(line, true) {
            let opens_fence = rest.trim_start().starts_with("```");
            out.msgs.extend(current.take());
            current = Some(Msg {
                label,
                content: rest.trim_start().to_string(),
                inline: !rest.trim_start().is_empty(),
            });
            if opens_fence {
                in_fence = true;
            }
            continue;
        }
        push_line(&mut current, &mut preamble, line);
    }
    out.msgs.extend(current.take());
    out.preamble = preamble.join("\n");
    out
}

/// 识别 Markdown 角色头：`## User`、`**User:**`、`> User:`、`- User:`。
/// 只接受关卡标记开头的行，裸 `User:` 由 `split_label_any` 处理。
fn markdown_header(line: &str) -> Option<(Label, &str)> {
    let t = line.trim_start();
    if !(t.starts_with('#')
        || t.starts_with("**")
        || t.starts_with('*')
        || t.starts_with("> ")
        || t.starts_with("- ")
        || t.starts_with("+ "))
    {
        return None;
    }
    let t = t.trim_start_matches('#').trim_start();
    let t = t.trim_start_matches(['*', '-', '>', '+', ' ']);
    const NAMES: [(&str, Label); 7] = [
        ("Assistant", Label::Assistant),
        ("System", Label::System),
        ("User", Label::User),
        ("Human", Label::User),
        ("AI", Label::Assistant),
        ("Question", Label::User),
        ("Answer", Label::Assistant),
    ];
    for (name, label) in NAMES {
        let Some(head) = t.get(..name.len()) else {
            continue;
        };
        if !head.eq_ignore_ascii_case(name) {
            continue;
        }
        let rest = &t[name.len()..];
        let rest = rest.trim_start_matches(['*', ' ', '\t']);
        let rest = rest.strip_prefix(':').unwrap_or(rest);
        let rest = rest.trim_start_matches(['*', ' ', '\t']);
        // `**User story**` 这种不是角色头：名字后必须紧跟 `*`/冒号/行尾。
        if !(rest.is_empty()
            || t[name.len()..].starts_with('*')
            || t[name.len()..].trim_start().starts_with(':'))
        {
            continue;
        }
        return Some((label, rest));
    }
    None
}

// ---------------------------------------------------------------------------
// 报告
// ---------------------------------------------------------------------------

fn diff_applied(before: &FormatReport, after: &FormatReport, changed: bool) -> Vec<AppliedFix> {
    use std::collections::BTreeMap;

    let mut counts: BTreeMap<&'static str, (usize, Severity, &str)> = BTreeMap::new();
    for f in &before.findings {
        let entry = counts.entry(f.code).or_insert((0, f.severity, &f.message));
        entry.0 += 1;
        if f.severity.rank() < entry.1.rank() {
            entry.1 = f.severity;
            entry.2 = &f.message;
        }
    }
    for f in &after.findings {
        if let Some(entry) = counts.get_mut(f.code) {
            entry.0 = entry.0.saturating_sub(1);
        }
    }
    let mut applied: Vec<AppliedFix> = counts
        .into_iter()
        .filter(|(_, (count, _, _))| *count > 0)
        .map(|(code, (count, _, message))| AppliedFix {
            code: code.to_string(),
            count,
            message: message.to_string(),
        })
        .collect();
    applied.sort_by(|a, b| {
        let rank = |code: &str| {
            before
                .findings
                .iter()
                .find(|f| f.code == code)
                .map_or(3, |f| f.severity.rank())
        };
        rank(&a.code)
            .cmp(&rank(&b.code))
            .then_with(|| a.code.cmp(&b.code))
    });
    if changed && applied.is_empty() {
        applied.push(AppliedFix {
            code: "TEXT_NORMALIZED".to_string(),
            count: 1,
            message: "空白/换行已规范化（检测器没有对应的 finding code）".to_string(),
        });
    }
    applied
}
