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

use crate::check::{
    KNOWN, Label, check, continuation_tail, fence_flags, label_name, parse_label, strip_cr,
};
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
    if after.error_count > 0 && notes.is_empty() {
        notes.push(format!(
            "修复后仍有 {} 个 error，属内容或结构歧义，未自动处理（见 after.findings）",
            after.error_count
        ));
    }
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
    // BOM 可能被空白挡在行首（`" \u{feff}User:"`），一律删掉，不能只认第一个字符。
    let normalized = text
        .replace('\u{feff}', "")
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    // 修复器只做规范化，不替调用方决定是否加文件末尾换行：原来有就保留，没有就不加。
    let final_newline = normalized.ends_with('\n');
    if normalized.trim().is_empty() {
        notes.push("输入为空，未做任何修改".to_string());
        return normalized;
    }
    // 元数据前缀在**解析前**去掉，否则 `[seg 3 · user] User: hi` 整行都不是标签，
    // 会被当成前言吞进 System。解析后的 strip_metadata 只兜底标签行内的前缀。
    let work = if options.strip_metadata {
        strip_seg_lines(&normalized)
    } else {
        normalized
    };

    // 续写标签（`English:` / `English: A`）是独立生成点，先切出来，
    // 否则会被当成上一块正文，安全修复会顺手把块间空行折掉。
    let (work, continuation) = if options.safe || options.structural {
        split_continuation(&work)
    } else {
        (work, None)
    };

    if work.trim().is_empty() && continuation.is_none() {
        notes.push("输入为空，未做任何修改".to_string());
        // 元数据剥离可能把「只有元数据」的输入变成空串；直接返回空串，
        // 否则空行数量每轮都会被 `lines()` 再收一次，收敛不了。
        return String::new();
    }

    let mut parsed = if options.structural {
        match convert_foreign(&work, notes) {
            Some(mut converted) => {
                // ChatML / Llama 3 的「第一个协议标记之前」可能仍是 G1 标签文本；
                // 当成前言包进 System 会让下一轮解析出不同的块结构。
                if !converted.preamble.trim().is_empty() {
                    let mut pre = parse_blocks(&converted.preamble, true, notes);
                    if !pre.msgs.is_empty() {
                        pre.msgs.append(&mut converted.msgs);
                        converted.msgs = pre.msgs;
                        converted.preamble = pre.preamble;
                    }
                }
                converted
            }
            None => parse_blocks(&work, true, notes),
        }
    } else {
        parse_blocks(&work, false, notes)
    };

    let last_idx = parsed.msgs.len().saturating_sub(1);
    for idx in 0..parsed.msgs.len() {
        normalize_msg(&mut parsed.msgs[idx], idx == last_idx, notes);
    }

    if options.strip_metadata {
        strip_metadata(&mut parsed.msgs);
        // 外部模板转换可能把 `[seg …]` 结尾追加到前言，前言尚未成块，单独清一遍。
        parsed.preamble = strip_seg_lines(&parsed.preamble);
    }

    let mut out = if options.structural {
        if !parsed.preamble.trim().is_empty() {
            let content = clean_text(&parsed.preamble);
            if !content.is_empty() {
                if parsed.msgs.is_empty() && continuation.is_some() {
                    // 纯续写：标签前的文本就是上下文，包装成 `User:` 会改变语义。
                    parsed.preamble = content;
                } else {
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
                    parsed.preamble.clear();
                    // 包装出来的块是新内容，还没跑过单块规范化（think hint / Tools / 行首缩进）。
                    let only = parsed.msgs.len() == 1;
                    normalize_msg(&mut parsed.msgs[0], only, notes);
                }
            }
        }
        transform(&mut parsed.msgs, notes, continuation.is_some());
        render(&parsed.msgs, &parsed.preamble, true)
    } else {
        render(&parsed.msgs, &parsed.preamble, false)
    };
    if let Some(cont) = &continuation {
        append_continuation(&mut out, cont);
    }
    if final_newline && !out.is_empty() {
        out.push('\n');
    }
    out
}

#[derive(Debug, Clone)]
struct Continuation {
    /// 规范写法（`English` / `German`…）。
    head: &'static str,
    /// 冒号后的 seed 内容，已去掉两侧空白；空表示留空续写。
    seed: String,
}

/// 把末尾续写标签从正文中切出来；标签前的空行由渲染统一补一个。
fn split_continuation(text: &str) -> (String, Option<Continuation>) {
    let lines: Vec<&str> = text.split('\n').collect();
    let flags = fence_flags(&lines);
    let Some((idx, head, tail)) = continuation_tail(&lines, &flags) else {
        return (text.to_string(), None);
    };
    let seed = tail.trim();
    (
        lines[..idx].join("\n").trim_end().to_string(),
        Some(Continuation {
            head,
            seed: seed.to_string(),
        }),
    )
}

fn append_continuation(out: &mut String, cont: &Continuation) {
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(cont.head);
    out.push(':');
    if !cont.seed.is_empty() {
        out.push(' ');
        out.push_str(&cont.seed);
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

fn render(msgs: &[Msg], preamble: &str, structural: bool) -> String {
    let mut blocks: Vec<String> = Vec::new();
    if !preamble.trim().is_empty() {
        let p = clean_text(preamble);
        if !p.is_empty() {
            blocks.push(balance_fences(p));
        }
    }
    for msg in msgs {
        blocks.push(balance_fences(render_msg(msg, structural)));
    }
    blocks.join("\n\n")
}

/// 渲染时保证正文里不会出现新标签行：正文首行若长得像标签，就保持在标签同一行；
/// 后续行若长得像标签，用空格接回上一行。角色标签只按行首识别，这样输出
/// 重新解析时块边界不变（`fix` 的固定点性质依赖这一点）。
fn render_msg(msg: &Msg, structural: bool) -> String {
    let name = label_name(msg.label);
    if msg.content.is_empty() {
        return format!("{name}:");
    }
    let mut lines = msg.content.split('\n');
    let first = lines.next().unwrap_or("");
    let inline = msg.inline || must_inline(msg) || line_starts_role(first, structural);
    let mut out = String::new();
    out.push_str(name);
    out.push(':');
    if inline {
        out.push(' ');
    } else {
        out.push('\n');
    }
    out.push_str(first);
    let mut in_fence = first.trim_start().starts_with("```");
    for line in lines {
        let fence_line = line.trim_start().starts_with("```");
        if !in_fence && !fence_line && line_starts_role(line, structural) {
            out.push(' ');
        } else {
            out.push('\n');
        }
        out.push_str(line);
        if fence_line {
            in_fence = !in_fence;
        }
    }
    out
}

/// 下一轮解析会把这行当成块边界/外部模板特征吗？是则渲染时不能让它落在行首。
/// 注意 `## User` / `> User:` 这类 Markdown 头会触发结构模式的模板转换。
fn line_starts_role(line: &str, structural: bool) -> bool {
    split_label_any(line, structural).is_some()
        || (structural && (markdown_header(line).is_some() || starts_foreign_marker(line)))
}

/// 行首的外部协议标记会触发整段模板转换；合并块后不能让它们落到行首。
fn starts_foreign_marker(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("<|im_start|>")
        || t.starts_with("<|start_header_id|>")
        || t.starts_with("<|begin_of_text|>")
}

/// 块内围栏必须是偶数个，否则重新解析时会把后面的块吞进代码块。
/// 一个未闭合的围栏补一行闭合；这是幂等的（补完就是偶数）。
fn balance_fences(block: String) -> String {
    let fences = block
        .lines()
        .filter(|line| {
            line.trim_start().starts_with("```")
                || parse_label(line).is_some_and(|(_, rest)| rest.trim_start().starts_with("```"))
        })
        .count();
    if fences % 2 == 1 {
        format!("{block}\n```")
    } else {
        block
    }
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
    let content_lines: Vec<&str> = msg.content.lines().collect();
    let start = content_lines
        .iter()
        .position(|line| !line.trim().is_empty())
        .unwrap_or(0);
    let end = content_lines
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .map_or(0, |i| i + 1);
    let had_fence_blank = msg.content.contains("```")
        && content_lines[start..end]
            .iter()
            .any(|line| line.trim().is_empty());

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

    // think hint 被移到句末后，行首可能留下一个空格（`(think) solve` → ` solve`）。
    // 内联块的首行不能带缩进，否则 `User:  solve` 要第二次修复才收敛。
    if msg.inline {
        content = match content.split_once('\n') {
            Some((first, rest)) => format!("{}\n{rest}", first.trim_start()),
            None => content.trim_start().to_string(),
        };
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
    // 移除 hint 后可能留下空白行；正文空行本来就在前面被过滤过，这里可以安全再过滤一次。
    let mut out = out_lines
        .into_iter()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_string();
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
                    if out.trim().is_empty() {
                        // hint 在行首：删除后残留的分隔空格也一起吃掉，否则会多出空行。
                        out.clear();
                        while chars.get(i).is_some_and(|c| c.is_whitespace()) {
                            i += 1;
                        }
                    } else if let Some(&next) = chars.get(i)
                        && !next.is_whitespace()
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

/// 解析前按行去掉 `[seg N · role]` 前缀；整行只有元数据时直接丢弃。
fn strip_seg_lines(text: &str) -> String {
    text.lines()
        .filter_map(|line| {
            let t = line.trim_start();
            if let Some(rest) = strip_seg_prefix(t) {
                let rest = rest.trim_start();
                (!rest.is_empty()).then(|| rest.to_string())
            } else {
                Some(line.to_string())
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
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

fn transform(msgs: &mut Vec<Msg>, notes: &mut Vec<String>, has_continuation: bool) {
    drop_empty(msgs);
    merge_system(msgs, notes);
    merge_same_role(msgs);
    reorder_instruction(msgs);
    // 重排会把两个同角色块变成相邻（Instruction/Instruction、Input/Input），
    // 必须再合并一次，否则下一轮修复才收敛。
    merge_same_role(msgs);
    if !has_continuation {
        ensure_generation_point(msgs);
    }
    // 合并/补生成点会改变「谁是最后一块」与正文边界，之前的归一化结果可能已失效；
    // 再跑一遍单块规范化，保证 fix 的输出就是下一轮的固定点。
    let last = msgs.len().saturating_sub(1);
    for (idx, msg) in msgs.iter_mut().enumerate() {
        normalize_msg(msg, idx == last, notes);
    }
    audit_residual(msgs, notes, has_continuation);
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

fn audit_residual(msgs: &[Msg], notes: &mut Vec<String>, has_continuation: bool) {
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
    if !has_continuation
        && let Some(pos) = msgs.iter().position(|m| m.label == Label::System)
        && msgs
            .get(pos + 1)
            .is_some_and(|m| m.label == Label::Assistant)
    {
        notes.push("`System:` 后直接跟 `Assistant:`（缺 `User:`）：未自动补造用户问题".to_string());
    }
}

// ---------------------------------------------------------------------------
// 外部模板转换
// ---------------------------------------------------------------------------

fn convert_foreign(text: &str, notes: &mut Vec<String>) -> Option<Parsed> {
    // 只有协议标记出现在行首（允许前面是别的协议标记）才按整段模板转换。
    // `User: <|im_start|>user…` 这种「G1 标签里嵌了外部标记」是内容，
    // 整段转换会把标签和正文顺序改掉。
    if marker_at_line_start(text, "<|im_start|>") {
        let parsed = convert_chatml(text, notes);
        notes.push("已把 ChatML 模板转换为 G1 plain".to_string());
        return Some(parsed);
    }
    if marker_at_line_start(text, "<|start_header_id|>") {
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

/// 标记是否位于行首；行首之前只允许出现其它协议标记/空白（Llama 3 的
/// `<|begin_of_text|>` 会和 `<|start_header_id|>` 连在同一行）。
fn marker_at_line_start(text: &str, marker: &str) -> bool {
    text.match_indices(marker).any(|(idx, _)| {
        let line_start = text[..idx].rfind('\n').map_or(0, |p| p + 1);
        let mut rest = text[line_start..idx].trim();
        loop {
            let mut stripped = false;
            for allowed in [
                "<|begin_of_text|>",
                "<|end_of_text|>",
                "<|endoftext|>",
                "<|eot_id|>",
                "<|im_end|>",
            ] {
                if let Some(next) = rest.strip_prefix(allowed) {
                    rest = next.trim();
                    stripped = true;
                }
            }
            if !stripped {
                break;
            }
        }
        rest.is_empty()
    })
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
                content: strip_protocol_markers(content)
                    .trim_matches('\n')
                    .to_string(),
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
    // 协议标记是模板脚手架，不是内容；残留任意一个都会让下一轮 `convert_foreign`
    // 再次命中，修复结果就不是固定点了。
    text.replace("<|begin_of_text|>", "")
        .replace("<|end_of_text|>", "")
        .replace("<|endoftext|>", "")
        .replace("<|eot_id|>", "")
        .replace("<|im_start|>", "")
        .replace("<|im_end|>", "")
        .replace("<|start_header_id|>", "")
        .replace("<|end_header_id|>", "")
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

/// 识别 Markdown 角色头：`## User`、`**User:**`、`> User:`。
/// 只接受关卡标记开头的行，裸 `User:` 由 `split_label_any` 处理。
/// 不把 `- User:` 当角色头——列表项在 prompt 里太常见，误判会吞掉正文。
fn markdown_header(line: &str) -> Option<(Label, &str)> {
    let t = line.trim_start();
    if !(t.starts_with('#') || t.starts_with("**") || t.starts_with('*') || t.starts_with("> ")) {
        return None;
    }
    let t = t.trim_start_matches('#').trim_start();
    let t = t.trim_start_matches(['*', '>', ' ']);
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
