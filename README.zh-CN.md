# rwkv-prompt-format

[English](README.md) | 简体中文

![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)
![Rust](https://img.shields.io/badge/rust-edition%202024-orange)
![Node](https://img.shields.io/badge/node-%E2%89%A518-green)
![WASM](https://img.shields.io/badge/wasm-203%20KB-purple)

RWKV G1/G1x 提示词**格式检查器 + 自动修复器**，独立于推理运行时：纯文本输入，
不加载模型、不碰 GPU、不读写磁盘。六种消费方式：

| 方式 | 适用 | 产物 |
|---|---|---|
| JSON CLI | 任何语言、脚本、CI | `rwkv-pf(.exe)` |
| C ABI | C/C++/Rust/任意 FFI 封装 | `rwkv_prompt_format.dll` / `.so` / `.dylib` |
| 薄封装 | Python / Node / Go / Java / C# | 见 `bindings/` |
| WASM | Node / 浏览器 / Edge / 任意 WASM 宿主 | `wasm/rwkv_prompt_format.wasm` + `wasm/index.mjs` |
| MCP | Qoder / Claude Desktop 等 AI agent | `mcp/server.mjs`（零依赖 stdio） |
| Skill | AI agent 的触发规则与工作流 | `skills/rwkv-prompt-format/` |

规则来自官方模板（[BlinkDL/RWKV-LM · RWKV7-G1x-templates.txt](https://github.com/BlinkDL/RWKV-LM/blob/main/RWKV-v7/RWKV7-G1x-templates.txt)），
而不是本仓的猜测。要点：

- 块之间**恰好一个空行**（`\n\n`）；空行不能出现在块内容里。
- 标签与关键字**大小写敏感**：`System:` / `User:` / `Assistant:` / `Instruction:` /
  `Input:` / `Response:` / `Tools:` / `<think` / `<think></think` / `(think)`。
- 生成点必须是 `Assistant:`（chat）或 `Response:`（instruction），且位于末尾。
- 官方材料模板是 `User:\n材料：…\n问题：…\n\nAssistant: {"answer": `（标签后换行）。

## 快速开始

```bash
git clone https://github.com/ai00-x/rwkv-prompt-format.git
cd rwkv-prompt-format
cargo build --release

# 检查（仍有 error 时退出码 1）
printf 'User: hi\n\n\nAssistant:' | ./target/release/rwkv-pf check --stdin

# 修复到 stdout
printf 'User: hi\n\n\nAssistant:' | ./target/release/rwkv-pf fix --stdin
```

没有 Rust 工具链？Node 18+ 可直接跑 WASM 版：

```bash
printf 'user : hi\n\n\nassistant : ' | node wasm/cli.mjs fix --stdin
```

AI agent 直接看 [MCP server](#mcp-server) 与 [Skill 与一句话安装](#skill-与一句话安装)。

## 构建

```bash
cd rwkv-prompt-format
cargo build --release
cargo fmt --all && cargo clippy --all-targets -- -D warnings   # 仓库硬规则

# WASM（需要 rustup target add wasm32-unknown-unknown）
node scripts/build-wasm.mjs          # 构建并复制到 wasm/
node scripts/build-wasm.mjs --check  # CI：产物过期则退出码 1
```

产物（`target/release/`）：

- `rwkv-pf` / `.exe` —— CLI（包名 `rwkv-prompt-format`，可执行文件取短名避免与库产物同名）
- `rwkv_prompt_format.dll`（Windows）/ `librwkv_prompt_format.so`（Linux）/
  `librwkv_prompt_format.dylib`（macOS）—— C ABI 动态库
- `rwkv_prompt_format.lib` / `librwkv_prompt_format.a` —— 静态/导入库
- 头文件：`include/rwkv_prompt_format.h`

WASM 产物固定为 `wasm/rwkv_prompt_format.wasm`（约 203 KB，无任何 import），
`wasm/index.mjs` 是零依赖封装。

## CLI

```bash
rwkv-pf check --file prompt.txt
cat prompt.txt | rwkv-pf check --stdin
rwkv-pf check --text "User: hi" --json
rwkv-pf fix   --file prompt.txt --in-place
cat chatml.txt | rwkv-pf fix --stdin > g1.txt
```

`check` 与 `fix` 都支持 `--file PATH` / `--text TEXT` / `--stdin`（三选一）、
`--json`、`--no-fail`。`fix` 另有：

| 参数 | 含义 |
|---|---|
| `--safe-only` | 只做安全修复，不改结构 |
| `--structural-only` | 只做结构修复（转换/合并/补生成点）；标签与空白仍按官方写法渲染 |
| `--strip-metadata` | 去掉行首 `[seg 12 · user]` 记忆元数据前缀（默认关，内容编辑） |
| `--in-place` | 写回 `--file`（需配 `--file`） |
| `--quiet` | 不打印 stderr 摘要 |

退出码：`0` 成功；`1` 检查/修复后仍有 error（`--no-fail` 可关）；`2` 读文件或参数错误。

默认 `fix` 把修复后的文本写到 stdout，把摘要写到 stderr；`--json` 时 stdout 是
`FixResult` JSON，摘要仍在 stderr。`--quiet` 关闭摘要，因此
`cat x | rwkv-pf fix --stdin > y` 是安全的。

### 人读输出

```
OK（0 error / 0 warn）· 格式 plain_chat · 生成点 L5 assistant
生成点：L5 `assistant:`（留空续写）
统计：33 字符 / 5 行 / User 1 · Assistant 1 · System 0 · Instruction 0 · 空行分隔 1 · 最大连续空行 1 · 元数据行 0
✓ 未发现格式问题
```

有 finding 时逐条给出 `L行号 [级别] 代码 说明`，下一行 `│` 是证据原文。

### JSON：`FormatReport`

```json
{
  "profile": "g1",
  "format": "plain_chat",
  "ok": false,
  "error_count": 1,
  "warning_count": 0,
  "info_count": 1,
  "generation_point": { "line": 5, "role": "assistant", "seeded": false },
  "stats": {
    "chars": 31, "lines": 5,
    "user_blocks": 1, "assistant_blocks": 1, "system_blocks": 0,
    "instruction_blocks": 0, "input_blocks": 0, "response_blocks": 0,
    "blank_separators": 1, "max_blank_run": 1,
    "metadata_lines": 0, "think_prefix": false, "function_call": false
  },
  "findings": [
    {
      "code": "EXTRA_BLANK_SEPARATOR",
      "severity": "error",
      "line": 3,
      "message": "块之间只能有一个空行（`\\n\\n`）；这里空了 2 行",
      "evidence": "User: hi"
    }
  ]
}
```

- `format`：`plain_chat` / `material_qa` / `instruction` / `instruction_chat` /
  `system_tools` / `chatml` / `markdown` / `jsonl` / `continuation` / `unknown`。
- `line` 从 1 开始；`0` 表示整段文本级问题。
- `severity`：`error` / `warn` / `info`。`ok == (error_count == 0)`。
- `code` 是稳定规则码（如 `MULTIPLE_SYSTEM`、`BLANK_LINE_IN_CONTENT`、
  `THINK_MALFORMED`），可直接用于 CI 白名单或统计。

### JSON：`FixResult`

```json
{
  "changed": true,
  "options": { "safe": true, "structural": true, "strip_metadata": false },
  "applied": [ { "code": "EXTRA_BLANK_SEPARATOR", "count": 1, "message": "..." } ],
  "notes": [],
  "before": { "...": "FormatReport" },
  "after": { "...": "FormatReport" },
  "text": "User: hi\n\nAssistant:"
}
```

`applied` 是**修复前后两次检测的差值**，不是修复器自报：只有真正消失的 error/warn
才会出现。`after` 是同一检测器对 `text` 的复检，`after.ok` 为 false 时说明还有
无法自动修复的内容（见 `after.findings` 与 `notes`）。

## 自动修复做什么

**安全修复**（`--safe-only`，`FIX_SAFE`）：

- BOM、CRLF/CR → LF；
- 标签大小写、缩进、行内多余空格、全角冒号、冒号前空格；
- 块间空行数量、内容里的空行、末尾空行、首部空行；
- `Tools:` 的大小写/冒号/位置；
- think 前缀（`<THINK>`、`<think >`、`<think>\n</think>` 等 → 官方形式）；
- `(think)` / `( think )` / `(Think a bit)` 等提示 → 句末唯一的官方写法；
- 未闭合的 ``` 围栏补一行闭合（`UNCLOSED_CODE_FENCE`），围栏内文本不动。

文件末尾换行**按原样保留**：原来有就保留，没有就不加。修复器不替调用方决定
文件级换行策略。

**结构修复**（`--structural-only`，`FIX_STRUCTURAL`）：

- ChatML（`<|im_start|>`…`<|im_end|>`）、Llama 3（`<|start_header_id|>`…）、
  JSONL（每行 `{"role","content"}` 或整个 JSON 数组/`{"messages":[…]}`）、
  Markdown（`## User`、`**User:**`、`> User:` 等）→ G1 plain；
  只转换**行首**的模板标记；`User: <|im_start|>user` 这种行内标记视为正文，
  不拆块（避免把内容改乱），检测器会报 `FOREIGN_MARKER`；
  `- User: …` 这类列表项也是正文，不当 Markdown 角色行；
- `Human:` / `AI:` / `Question:` / `Answer:` → 官方标签；
- 多个 `System:` 合并为一个（含 `Tools:` 时置顶并给出复核说明）；
- 连续同角色块合并；
- Instruction 块重排为 `System → Instruction → Input → Response`；
- 末尾缺生成点时补 `Assistant:` / `Response:`；
- 无任何标签的纯文本 → `User:` + 生成点；有标签前的散落文本 → 并入 `System:`；
- 末尾续写标签（`English:` / `English: A principios` 等 12 种语言名）单独保留为
  生成点，不与上一块的空行折叠；检测器把「对话 + 末尾续写标签」归为
  `format=continuation`。

**不做**的事（有意为之）：

- 不改写语义内容、不摘要、不翻译；`--strip-metadata` 是唯一的内容编辑，默认关。
- 不把 `Assistant:` 后面已经 seed 的内容（如 `{"answer": `）并到别的块，也不吃掉
  末尾 seed 空格；`Assistant:\n材料…` 这类原布局会保留。
- 不转换 Llama 2 `[INST]`、不猜没见过的私有模板（会给出 `notes` 说明）；
- 不保证语义上「正确」——它保证**符合官方模板的语法**。

修复器是确定性的、幂等的：`fix(fix(x)) == fix(x)`，测试覆盖所有转换路径。

## C ABI

```c
#include "rwkv_prompt_format.h"

char *json = rwkv_pf_check(text);            /* FormatReport JSON */
char *fixed = rwkv_pf_fix_text(text, RWKV_PF_FIX_SAFE | RWKV_PF_FIX_STRUCTURAL);
if (fixed) { fputs(fixed, stdout); }
rwkv_pf_free_string(fixed);                  /* 必须用本库释放，不能 free() */
rwkv_pf_free_string(json);
```

| 符号 | 说明 |
|---|---|
| `const char *rwkv_pf_version(void)` | 版本号，静态内存，不要释放 |
| `char *rwkv_pf_check(const char *text)` | 返回报告 JSON；错误返回 `{"error":"…"}` |
| `char *rwkv_pf_fix(const char *text, uint32_t flags)` | 返回 `FixResult` JSON（含 `text`） |
| `char *rwkv_pf_fix_text(const char *text, uint32_t flags)` | 只返回修复文本；失败返回 NULL |
| `void rwkv_pf_free_string(char *ptr)` | 释放上述返回串；NULL 是 no-op |

- 全部是纯函数，可并发调用；内部 `catch_unwind`，panic 不会跨 FFI 边界。
- 输入必须是有效 UTF-8；JSON 接口对 NULL/非 UTF-8 返回 `{"error":…}`，便于封装统一处理。
- 所有堆字符串都由本库分配、必须由本库释放（Windows 下跨 CRT 的 `free()` 会崩）。

## 多语言封装

| 语言 | 文件 | 传输 | 状态 |
|---|---|---|---|
| Python | `bindings/python/rwkv_prompt_format.py` | ctypes | ✅ 已测（Python 3.12；兼容 3.8+） |
| Node.js | `bindings/node/index.js` + `index.d.ts` | [koffi](https://koffi.dev) FFI | ✅ 已测（Node 22 + koffi 3.3.3） |
| Go | `bindings/go/` | JSON CLI（`os/exec`，无 cgo） | ✅ 已测（Go 1.25） |
| Java | `bindings/java/ai00x/rwkv/RwkvPromptFormat.java` | JNA | ⚠️ 未运行验证（本机无 JDK） |
| C# | `bindings/csharp/RwkvPromptFormat.cs` | P/Invoke | ⚠️ 未运行验证（本机无 .NET） |
| 其它 | CLI | `--json` | 任何能起子进程的语言 |

```python
from rwkv_prompt_format import check, fix          # RWKV_PROMPT_FORMAT_LIB 可指定库路径
report = check(text)
if not report["ok"]:
    text = fix(text)["text"]
```

```js
const pf = require('./bindings/node');             // 需要 pnpm add koffi
const report = pf.check(text);
const fixed = pf.fixText(text);
```

```go
import rwkvpf "github.com/ai00-x/rwkv-prompt-format/bindings/go"

report, err := rwkvpf.Check(text)                  // RWKV_PROMPT_FORMAT_CLI 或 PATH
fixed, err := rwkvpf.FixText(text, rwkvpf.FixAll)
```

```java
var report = RwkvPromptFormat.checkJson(text);     // JNA + -Djna.library.path=…
var fixed  = RwkvPromptFormat.fixText(text, RwkvPromptFormat.FIX_ALL);
```

```csharp
string report = RwkvPromptFormat.CheckJson(text);
string fixed  = RwkvPromptFormat.FixText(text);
```

## WASM

同一份 Rust 核心编译成 `wasm32-unknown-unknown`，复用 C ABI 的
`rwkv_pf_check/fix/fix_text/free_string`，另加三个只给 WASM 用的导出：

| 导出 | 说明 |
|---|---|
| `rwkv_pf_wasm_alloc(len) -> ptr` | 分配 `len` 字节（零填充），给输入串用 |
| `rwkv_pf_wasm_free(ptr, len)` | 释放输入缓冲区 |
| `rwkv_pf_wasm_abi() -> u32` | ABI 版本，当前 `1` |

- 模块**没有任何 import**，只导出 `memory` 与上述函数；不需要 wasm-bindgen。
- 输入缓冲末尾必须是 NUL（`index.mjs` 自动处理）；返回串用
  `rwkv_pf_free_string` 释放。
- 体积 202,998 bytes，sha256 `c0fe38e101b40986b4c99cf345b07f30ed50df24d9d0cb5bc033724b21ffbed6`；
  `wasm-release` profile 使用 `opt-level="z"` + LTO + `panic="abort"` + strip。

Node：

```js
import { check, fix, fixText, FIX_ALL } from './wasm/index.mjs';

const report = await check(text);
const fixed  = await fix(text, FIX_ALL);
const text2  = await fixText(text, FIX_ALL);
```

Node 自动加载同目录的 `.wasm`。浏览器/Edge：

```js
import { init, check } from './wasm/index.mjs';
await init(new URL('./rwkv_prompt_format.wasm', import.meta.url));
const report = await check(text);
```

`init(source)` 接受 URL/路径字符串、`Response`、`ArrayBuffer`、TypedArray 或
`WebAssembly.Module`。flags：`FIX_SAFE=1`、`FIX_STRUCTURAL=2`、
`FIX_STRIP_METADATA=4`、`FIX_ALL=3`。CLI：

```bash
node wasm/cli.mjs check --stdin --json < prompt.txt
node wasm/cli.mjs fix --stdin < prompt.txt
```

与 Rust CLI 的 `check` / `fix --json` 输出**逐字段一致**（30 组输入 × 4 档参数），
退出码也一致。浏览器加载路径按标准 API 实现，但本机没有真实浏览器环境，只在
Node 22 中验证了 bytes / ArrayBuffer / Response / URL / Module 五种 `init` 入口。

## MCP server

`mcp/server.mjs` 是零依赖 stdio MCP server（Node ≥ 18），把同一份 WASM 暴露成
三个工具：

| 工具 | 参数 | 作用 |
|---|---|---|
| `check_prompt_format` | `{ text }` | 返回 `FormatReport` |
| `fix_prompt_format` | `{ text, mode?: "safe"/"structural"/"both", strip_metadata?: boolean }` | 返回 `FixResult`（含 `text`） |
| `get_prompt_format_rules` | `{}` | 返回压缩版官方规则 |

资源：`rwkv-prompt-format://rules`（`text/markdown`）。工具结果同时给
`content[].text`（人类可读 JSON）与 `structuredContent`（机器可读）；参数错误
返回 `isError: true`，未知工具返回 JSON-RPC `-32602`。每个工具都带 `title`、
`annotations`（check/rules 只读、fix 幂等且非破坏）与 `outputSchema`，
`initialize` 还会返回一段 `instructions`，便于客户端直接提示 agent 走
check → fix → 复核流程。

配置（安装器会自动写这一段）：

```json
{
  "mcpServers": {
    "rwkv-prompt-format": {
      "command": "node",
      "args": ["/path/to/rwkv-prompt-format/mcp/server.mjs"]
    }
  }
}
```

- 传输是 **newline-delimited JSON-RPC 2.0**（MCP stdio 规范），不是
  `Content-Length` 帧；stdout 只走协议，日志全部走 stderr。
- 支持协议版本 `2024-11-05` / `2025-03-26` / `2025-06-18`，未知版本回落
  `2025-06-18`；输入上限 8 MiB。
- 纯文本、无网络、不读用户文件；可并发，不保存会话状态。

## Skill 与一句话安装

`skills/rwkv-prompt-format/` 是给 AI agent 的 skill：`SKILL.md` 说明何时检查、
check → fix → verify 的工作流、硬规则和禁止事项；`references/rules.md` 是完整规则
与 finding code 参考。skill 附带的 `wasm/cli.mjs` 可在 MCP 不可用时兜底。

安装器 `scripts/install-agent.mjs` 做两件事：

1. 把 skill、MCP server、WASM 复制成自包含目录（不依赖本仓路径）；
2. 在 Qoder 配置里合并 `mcpServers["rwkv-prompt-format"]`（保留其它键，
   写前备份，原子替换；配置 JSON 损坏时**不写任何文件**）。

| scope | skill 安装到 | MCP 配置写到 |
|---|---|---|
| `user`（默认） | `~/.qoder-cn/skills/rwkv-prompt-format/` | `~/.qoder-cn/settings.json` |
| `project` | `<project>/.qoder/skills/rwkv-prompt-format/` | `<project>/.qoder/settings.json` |
| `local` | `<project>/.qoder/skills/rwkv-prompt-format/` | `<project>/.qoder/settings.local.json` |

一句话安装（把这段发给 Qoder/任意 agent）：

> 运行 `node <path-to>/rwkv-prompt-format/scripts/install-agent.mjs --scope user --yes`，
> 然后提醒我执行 `/mcp reload` 和 `/skills reload`；之后处理 RWKV 提示词时先用
> `check_prompt_format` 检查，必要时用 `fix_prompt_format`（`mode="both"`）修复并确认
> `after.ok`。

或者自己跑：

```bash
node scripts/install-agent.mjs --scope user --yes
node scripts/install-agent.mjs --scope user --dry-run          # 只看计划
node scripts/install-agent.mjs --scope user --uninstall --yes   # 卸载
```

安装后必须重载：`/mcp reload`（MCP）与 `/skills reload`（或重启会话）。
卸载只删安装器自己写入的 MCP 条目和 skill 目录；同名但指向别处的条目会保留并以
退出码 1 提示。

## 能力边界

- 只检查**格式**，不判断回答对错；也不加载模型做 perplexity。
- 只支持 G1/G1x 官方模板。ChatML/Llama 3/JSONL/Markdown 是**输入**格式，会被转成
  G1；其它模板（Llama 2、Mistral、私有 chat template）不猜，也不会把行内出现的
  `<|im_start|>` 当作结构标记。
- 规则码是稳定 API；`message` / `evidence` 文案可能改进，不建议用于断言。
- 修复后仍可能残留 error（例如 `Assistant:` 开头、混合 instruction/chat），
  报告会逐条列出，`notes` 会说明为何不自动处理。
- CLI 每次启动约数毫秒；批量场景用 C ABI/ctypes/koffi/WASM，不要循环起进程。
- WASM 是纯 `wasm32-unknown-unknown` 模块，不依赖 WASI，不能读写文件或网络；
  调用方负责加载/传参。浏览器加载路径未在真实浏览器中跑过；如需更小体积可再
  接 `wasm-opt`（本机未安装）。
- MCP 只有 stdio 传输（没有 HTTP/SSE），需要 Node ≥ 18；工具输入上限 8 MiB。
- C ABI/WASM 的输入以 NUL 结尾，因此文本不能含 `\0`；WASM 封装与 MCP 会显式
  拒绝并报错，不会静默截断。
- 安装器目前只写 Qoder 的 `settings.json` / `settings.local.json` 与 skill 目录；
  Claude Desktop/Cursor 等其它客户端需要手动复制 MCP 配置。安装后必须
  `/mcp reload` 与 `/skills reload`（或重启会话）才生效。

## 测试

以下是 2026-10-21 的完整验证基线（一次性测试脚本，验证完成后已删除）：

| 套件 | 规模 | 覆盖 |
|---|---|---|
| `check` 矩阵 | 73 例 | 每种 format 的 finding code/severity/line、生成点、stats 交叉校验、确定性 |
| `fix` 精确输出 | 34 例 | safe / structural / metadata 三档的逐字输出、外部模板转换 |
| `fix` 模式不变量 | 44 例 | 三档参数下 `changed/after/ok/applied/notes` 一致性、幂等 |
| 确定性模糊 | 6000 例 | 四档参数各 1500 例随机标签/冒号/空白/围栏/外部模板；`fix(fix(x)) == fix(x)`、`after == check(text)`、残错必有 notes、输出无 CR/BOM |
| CLI | 27 项 | 退出码 0/1/2、三种输入源、`--json`、`--quiet`、`--in-place`、非法 UTF-8、Unicode 与 1 MB 往返 |
| C ABI | 1 套 | FFI 与 CLI 的 JSON 逐字段一致、8 线程并发、3000 次调用内存稳定、NULL/非法 UTF-8、静态版本指针、1 MB 输入 |
| 语言封装 | 3 套 | Python、Node(koffi)、Go 的 check/fix/幂等/Unicode/大输入 parity |
| WASM（含封装） | 304 项 | 30 组输入 × 4 档参数与 Rust CLI 的 check/fix JSON 逐字段一致、退出码一致、幂等；五种 `init` 入口；1 MB；100 并发；5000 次 alloc/free 内存稳定；无 import |
| MCP | 27 项 | initialize/协议版本、tools/list、三个工具、resources、错误码、结构化结果、notification 不回包、batch 拒绝、stdout 无噪声、9 KB 输入 |
| Skill 安装器 | 48 项 | user/project/local 三 scope、配置合并保留其它键、幂等重跑、坏 JSON 不落盘、卸载所有权、安装后 MCP 实跑 |

以上一次性验证合计 **379 项，全部 0 失败**。Java/C# 封装因本机无 JDK/.NET 未运行
验证；浏览器 WASM 未在真实浏览器中运行。

其中可重复执行的关键路径已固化为常驻门禁 `scripts/verify.mjs`（当前 **74 项**，
`--quick` 为 34 项），不依赖临时文件，也不写仓库：

| 套件 | 覆盖 |
|---|---|
| Rust ↔ WASM parity | 15 组语料（chat/messy/ChatML/Llama 3/JSONL/Markdown/材料问答/think/围栏/BOM+CRLF/续写/元数据/空文本/纯文本/64 KB Unicode）× check 与 safe/structural/both 三档 fix；`changed`/`after == check(text)`/残错必有 notes/幂等 |
| MCP | initialize/instructions、tools/list（title/annotations/outputSchema）、check/fix、resources、未知方法/工具、非法 mode、NUL 拒绝、notification 不回包、ping |
| 安装器 | user scope 安装、配置合并保留无关键、幂等重跑、安装后 MCP server 实跑、坏 JSON 不落盘、非自有条目不删、卸载清理 |
| 元数据 | `--strip-metadata` 两栈逐字段一致 |

```bash
cargo fmt --all -- --check && cargo clippy --all-targets -- -D warnings
cargo build --release
node scripts/build-wasm.mjs --check
node scripts/verify.mjs
printf 'User: hi\n\n\nAssistant:' | ./target/release/rwkv-pf fix --stdin
node wasm/cli.mjs check --text "User: hi" --json
```

`verify.mjs` 需要 `target/release/rwkv-pf` 和已提交的 WASM；CI 把上述命令按序
串起来即可，当前未附 workflow 文件。

## 贡献

欢迎 issue 与 PR。提交前请跑 `cargo fmt --all`、`cargo clippy --all-targets -- -D warnings`、
`node scripts/build-wasm.mjs --check`、`node scripts/verify.mjs`。规则码是公开 API，
不要随意改名。

## License

MIT OR Apache-2.0。见 [LICENSE-MIT](LICENSE-MIT) 与 [LICENSE-APACHE](LICENSE-APACHE)。
