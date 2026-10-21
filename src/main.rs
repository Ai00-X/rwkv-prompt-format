//! `rwkv-prompt-format` CLI：检查 / 自动修复，JSON 输出可被任何语言消费。

use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{ArgGroup, Args, Parser, Subcommand};
use rwkv_prompt_format::{FixOptions, FixResult, FormatReport, Severity, check, fix};

#[derive(Debug, Parser)]
#[command(
    name = "rwkv-prompt-format",
    version,
    about = "RWKV G1/G1x 提示词格式检查与自动修复（纯文本，不加载模型）",
    after_help = "示例:\n  rwkv-prompt-format check --file prompt.txt\n  cat prompt.txt | rwkv-prompt-format fix --stdin\n  rwkv-prompt-format fix --file prompt.txt --in-place"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// 检查格式；有 error 时退出码 1（--no-fail 可关）。
    Check(CheckArgs),
    /// 自动修复；默认把修复后的文本写到 stdout。
    Fix(FixArgs),
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("source")
        .required(true)
        .args(["file", "text", "stdin"])
))]
struct SourceArgs {
    /// 从文件读取。
    #[arg(long, value_name = "PATH")]
    file: Option<PathBuf>,
    /// 直接给一段文本。
    #[arg(long, value_name = "TEXT")]
    text: Option<String>,
    /// 从 stdin 读取。
    #[arg(long)]
    stdin: bool,
}

#[derive(Debug, Args)]
struct CheckArgs {
    #[command(flatten)]
    source: SourceArgs,
    /// 输出 JSON（schema 见 README）。
    #[arg(long)]
    json: bool,
    /// 即使有 error 也返回 0。
    #[arg(long)]
    no_fail: bool,
}

#[derive(Debug, Args)]
struct FixArgs {
    #[command(flatten)]
    source: SourceArgs,
    /// 只做安全修复（标签/空白/空行/think/Tools），不改结构。
    #[arg(long, conflicts_with = "structural_only")]
    safe_only: bool,
    /// 只做结构修复（转换/合并/补生成点），不额外做安全归一。
    #[arg(long)]
    structural_only: bool,
    /// 去掉 `[seg 12 · user]` 这类记忆元数据前缀（内容编辑，默认关）。
    #[arg(long)]
    strip_metadata: bool,
    /// 把结果写回 --file 指定的文件。
    #[arg(long, requires = "file")]
    in_place: bool,
    /// 输出 FixResult JSON（含 before/after/applied/text）。
    #[arg(long)]
    json: bool,
    /// 不打印修复摘要（stderr）。
    #[arg(long)]
    quiet: bool,
    /// 修复后仍有 error 也返回 0。
    #[arg(long)]
    no_fail: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("错误：{err}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode, String> {
    match cli.command {
        Command::Check(args) => run_check(args),
        Command::Fix(args) => run_fix(args),
    }
}

fn run_check(args: CheckArgs) -> Result<ExitCode, String> {
    let text = read_source(&args.source)?;
    let report = check(&text);
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
        );
    } else {
        print_report(&report);
    }
    Ok(if !report.ok && !args.no_fail {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

fn run_fix(args: FixArgs) -> Result<ExitCode, String> {
    let text = read_source(&args.source)?;
    let options = if args.safe_only {
        FixOptions {
            safe: true,
            structural: false,
            strip_metadata: args.strip_metadata,
        }
    } else if args.structural_only {
        FixOptions {
            safe: false,
            structural: true,
            strip_metadata: args.strip_metadata,
        }
    } else {
        FixOptions {
            safe: true,
            structural: true,
            strip_metadata: args.strip_metadata,
        }
    };
    let result = fix(&text, &options);

    if args.in_place {
        let path = args.source.file.as_ref().ok_or("--in-place 需要 --file")?;
        std::fs::write(path, &result.text)
            .map_err(|e| format!("写 {} 失败：{e}", path.display()))?;
    }

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?
        );
    } else if !args.in_place {
        print!("{}", result.text);
    }

    if !args.quiet {
        print_fix_summary(&result, args.in_place);
    }

    Ok(if !result.after.ok && !args.no_fail {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

fn read_source(args: &SourceArgs) -> Result<String, String> {
    if let Some(path) = &args.file {
        return std::fs::read_to_string(path)
            .map_err(|e| format!("读 {} 失败：{e}", path.display()));
    }
    if let Some(text) = &args.text {
        return Ok(text.clone());
    }
    let mut text = String::new();
    std::io::stdin()
        .read_to_string(&mut text)
        .map_err(|e| format!("读 stdin 失败：{e}"))?;
    Ok(text)
}

fn print_report(report: &FormatReport) {
    println!("{}", report.summary_line());
    if let Some(gp) = &report.generation_point {
        println!(
            "生成点：L{} `{}:`（{}）",
            gp.line,
            gp.role,
            if gp.seeded {
                "已 seed"
            } else {
                "留空续写"
            }
        );
    } else {
        println!("生成点：无（模型没有明确的续写位置）");
    }
    let s = &report.stats;
    println!(
        "统计：{} 字符 / {} 行 / User {} · Assistant {} · System {} · Instruction {} · \
         空行分隔 {} · 最大连续空行 {} · 元数据行 {}{}",
        s.chars,
        s.lines,
        s.user_blocks,
        s.assistant_blocks,
        s.system_blocks,
        s.instruction_blocks,
        s.blank_separators,
        s.max_blank_run,
        s.metadata_lines,
        if s.think_prefix {
            " · think"
        } else if s.function_call {
            " · tools"
        } else {
            ""
        }
    );
    print_findings(report);
}

fn print_findings(report: &FormatReport) {
    if report.findings.is_empty() {
        println!("✓ 未发现格式问题");
        return;
    }
    for finding in &report.findings {
        let loc = if finding.line == 0 {
            "  -  ".to_string()
        } else {
            format!("L{:<4}", finding.line)
        };
        println!(
            "  {loc} [{}] {:<28} {}",
            finding.severity.tag(),
            finding.code,
            finding.message
        );
        if !finding.evidence.is_empty() {
            println!("           │ {}", finding.evidence);
        }
    }
}

fn print_fix_summary(result: &FixResult, in_place: bool) {
    let status = if result.changed {
        "已修改"
    } else {
        "未修改"
    };
    eprintln!(
        "修复：{status} · 修复项 {} 类 · 残余 {} error / {} warn{}",
        result.applied.len(),
        result.after.error_count,
        result.after.warning_count,
        if in_place { " · 已写回文件" } else { "" }
    );
    for item in &result.applied {
        let severity = result
            .before
            .findings
            .iter()
            .find(|f| f.code == item.code)
            .map(|f| f.severity)
            .unwrap_or(Severity::Info);
        eprintln!(
            "  [{}] {:<28} ×{:<3} {}",
            severity.tag(),
            item.code,
            item.count,
            item.message
        );
    }
    for note in &result.notes {
        eprintln!("  说明：{note}");
    }
    if result.after.error_count > 0 {
        eprintln!("  残余 error（无法自动修复，见 --json 的 after.findings）：");
        for finding in result
            .after
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Error)
            .take(8)
        {
            eprintln!(
                "    L{} {}: {}",
                finding.line, finding.code, finding.message
            );
        }
    }
}
