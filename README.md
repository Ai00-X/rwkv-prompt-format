# rwkv-prompt-format

English | [简体中文](README.zh-CN.md)

![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)
![Rust](https://img.shields.io/badge/rust-edition%202024-orange)
![Node](https://img.shields.io/badge/node-%E2%89%A518-green)
![WASM](https://img.shields.io/badge/wasm-203%20KB-purple)

A **format checker and auto-fixer for RWKV G1/G1x prompts**. It is independent of the
inference runtime: it takes plain text, never loads a model, never touches a GPU, and
never reads or writes files. Six ways to consume it:

| Form | Best for | Artifact |
|---|---|---|
| JSON CLI | Any language, scripts, CI | `rwkv-pf(.exe)` |
| C ABI | C/C++/Rust or any FFI wrapper | `rwkv_prompt_format.dll` / `.so` / `.dylib` |
| Thin bindings | Python / Node / Go / Java / C# | see `bindings/` |
| WASM | Node / browsers / edge / any WASM host | `wasm/rwkv_prompt_format.wasm` + `wasm/index.mjs` |
| MCP | Qoder, Claude Desktop, and other AI agents | `mcp/server.mjs` (zero-dependency stdio) |
| Skill | Trigger rules and workflow for AI agents | `skills/rwkv-prompt-format/` |

The rules come from the official templates
([BlinkDL/RWKV-LM · RWKV7-G1x-templates.txt](https://github.com/BlinkDL/RWKV-LM/blob/main/RWKV-v7/RWKV7-G1x-templates.txt)),
not from guesses. The essentials:

- Blocks are separated by **exactly one blank line** (`\n\n`); a blank line must never
  appear inside block content.
- Labels and keywords are **case-sensitive**: `System:` / `User:` / `Assistant:` /
  `Instruction:` / `Input:` / `Response:` / `Tools:` / `<think` / `<think></think` /
  `(think)`.
- The generation point must be `Assistant:` (chat) or `Response:` (instruction), and it
  must be last.
- The official material template is
  `User:\n材料：…\n问题：…\n\nAssistant: {"answer": ` (content starts on the next line).

## Quick start

```bash
git clone https://github.com/ai00-x/rwkv-prompt-format.git
cd rwkv-prompt-format
cargo build --release

# Check a prompt (exit code 1 if errors remain)
printf 'User: hi\n\n\nAssistant:' | ./target/release/rwkv-pf check --stdin

# Fix it to stdout
printf 'User: hi\n\n\nAssistant:' | ./target/release/rwkv-pf fix --stdin
```

No Rust toolchain? The WASM build runs on Node 18+ with no dependencies:

```bash
printf 'user : hi\n\n\nassistant : ' | node wasm/cli.mjs fix --stdin
```

For AI agents, jump to [MCP server](#mcp-server) or
[agent skill and one-prompt install](#agent-skill-and-one-prompt-install).

## Build

```bash
cd rwkv-prompt-format
cargo build --release
cargo fmt --all && cargo clippy --all-targets -- -D warnings   # repository gate

# WASM (requires: rustup target add wasm32-unknown-unknown)
node scripts/build-wasm.mjs          # build and copy into wasm/
node scripts/build-wasm.mjs --check  # CI: exit 1 if the committed artifact is stale
```

Artifacts in `target/release/`:

- `rwkv-pf` / `.exe` — CLI (the package is `rwkv-prompt-format`; the binary uses the
  short name so it does not collide with the library artifact)
- `rwkv_prompt_format.dll` (Windows) / `librwkv_prompt_format.so` (Linux) /
  `librwkv_prompt_format.dylib` (macOS) — C ABI dynamic library
- `rwkv_prompt_format.lib` / `librwkv_prompt_format.a` — static/import library
- header: `include/rwkv_prompt_format.h`

The WASM artifact is always `wasm/rwkv_prompt_format.wasm` (about 203 KB, no imports);
`wasm/index.mjs` is a zero-dependency wrapper.

## CLI

```bash
rwkv-pf check --file prompt.txt
cat prompt.txt | rwkv-pf check --stdin
rwkv-pf check --text "User: hi" --json
rwkv-pf fix   --file prompt.txt --in-place
cat chatml.txt | rwkv-pf fix --stdin > g1.txt
```

Both `check` and `fix` accept `--file PATH` / `--text TEXT` / `--stdin` (exactly one),
`--json`, and `--no-fail`. `fix` also supports:

| Flag | Meaning |
|---|---|
| `--safe-only` | Safe repairs only; never changes structure |
| `--structural-only` | Structural repairs only (convert/merge/add generation point); labels and whitespace are still rendered canonically |
| `--strip-metadata` | Drop leading `[seg 12 · user]` memory-metadata prefixes (off by default; this edits content) |
| `--in-place` | Write back to `--file` (requires `--file`) |
| `--quiet` | Suppress the stderr summary |

Exit codes: `0` success; `1` errors remain after check/fix (`--no-fail` disables);
`2` file or argument error.

By default `fix` writes the repaired text to stdout and the summary to stderr. With
`--json`, stdout is a `FixResult` JSON document and the summary stays on stderr.
`--quiet` removes the summary, so `cat x | rwkv-pf fix --stdin > y` is safe.

### Human-readable output

```
OK（0 error / 0 warn）· 格式 plain_chat · 生成点 L5 assistant
生成点：L5 `assistant:`（留空续写）
统计：33 字符 / 5 行 / User 1 · Assistant 1 · System 0 · Instruction 0 · 空行分隔 1 · 最大连续空行 1 · 元数据行 0
✓ 未发现格式问题
```

Findings are printed one per line as `L<line> [severity] <code> <message>`, followed by
a `│` line with the original evidence.

> The human-readable summary and finding messages are currently in Chinese. Use
> `--json` and `findings[].code` for locale-independent automation.

### JSON: `FormatReport`

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

- `format`: `plain_chat` / `material_qa` / `instruction` / `instruction_chat` /
  `system_tools` / `chatml` / `markdown` / `jsonl` / `continuation` / `unknown`.
- `line` is 1-based; `0` means a whole-text finding.
- `severity`: `error` / `warn` / `info`. `ok == (error_count == 0)`.
- `code` is a stable rule code (for example `MULTIPLE_SYSTEM`,
  `BLANK_LINE_IN_CONTENT`, `THINK_MALFORMED`) and is safe to use in CI allowlists or
  metrics.

### JSON: `FixResult`

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

`applied` is the **difference between two checks** (before and after), not a
self-report from the fixer: only errors/warnings that actually disappeared appear
there. `after` is the same checker run against `text`. If `after.ok` is false,
something could not be repaired automatically; see `after.findings` and `notes`.

## What the fixer does

**Safe repairs** (`--safe-only`, `FIX_SAFE`):

- BOM, CRLF/CR → LF;
- label case, indentation, extra inline spaces, full-width colons, spaces before colons;
- blank-line counts between blocks, blank lines inside content, trailing/leading blank
  lines;
- `Tools:` case/colon/position;
- think prefixes (`<THINK>`, `<think >`, `<think>\n</think>`, etc. → the official form);
- think hints (`(think)`, `( think )`, `(Think a bit)`, etc. → one canonical hint at the
  end of the sentence);
- adds the closing line for an unclosed ``` fence (`UNCLOSED_CODE_FENCE`); text inside
  the fence is untouched.

The final newline is **preserved as-is**: if the input had one, it stays; if not, none is
added. The fixer does not make file-level newline decisions for the caller.

**Structural repairs** (`--structural-only`, `FIX_STRUCTURAL`):

- ChatML (`<|im_start|>`…`<|im_end|>`), Llama 3 (`<|start_header_id|>`…), JSONL (one
  `{"role","content"}` object per line, or a JSON array / `{"messages":[…]}`), and
  Markdown (`## User`, `**User:**`, `> User:`, etc.) → G1 plain;
  only **line-leading** template markers are converted. An inline marker such as
  `User: <|im_start|>user` is treated as content and not split into blocks (to avoid
  mangling text); the checker reports `FOREIGN_MARKER`. List items like `- User: …`
  are content, not Markdown role lines;
- `Human:` / `AI:` / `Question:` / `Answer:` → official labels;
- multiple `System:` blocks are merged into one (when `Tools:` is present it is moved to
  the top and a review note is emitted);
- consecutive same-role blocks are merged;
- instruction blocks are reordered to `System → Instruction → Input → Response`;
- a missing generation point is appended (`Assistant:` / `Response:`);
- tagless plain text → `User:` + generation point; stray text before the first label is
  folded into `System:`;
- a trailing continuation label (`English:`, `English: A principios`, and 12 language
  names in total) is kept as its own generation point and is not merged with the
  preceding block. The checker classifies "dialogue + trailing continuation label" as
  `format=continuation`.

**Deliberately not done**:

- No semantic rewriting, summarizing, or translation. `--strip-metadata` is the only
  content edit, and it is off by default.
- Seeded content after `Assistant:` (such as `{"answer": `) is never merged into another
  block, and the trailing seed space is preserved; layouts like `Assistant:\n材料…` stay
  as they are.
- Llama 2 `[INST]` and unknown private templates are not guessed (a `notes` entry
  explains why).
- It does not guarantee semantic correctness — it guarantees **syntax that matches the
  official template**.

The fixer is deterministic and idempotent: `fix(fix(x)) == fix(x)`, with tests covering
every conversion path.

## C ABI

```c
#include "rwkv_prompt_format.h"

char *json = rwkv_pf_check(text);            /* FormatReport JSON */
char *fixed = rwkv_pf_fix_text(text, RWKV_PF_FIX_SAFE | RWKV_PF_FIX_STRUCTURAL);
if (fixed) { fputs(fixed, stdout); }
rwkv_pf_free_string(fixed);                  /* must be freed by this library, not free() */
rwkv_pf_free_string(json);
```

| Symbol | Description |
|---|---|
| `const char *rwkv_pf_version(void)` | Version string in static memory; do not free |
| `char *rwkv_pf_check(const char *text)` | Returns report JSON; errors return `{"error":"…"}` |
| `char *rwkv_pf_fix(const char *text, uint32_t flags)` | Returns `FixResult` JSON (including `text`) |
| `char *rwkv_pf_fix_text(const char *text, uint32_t flags)` | Returns only the repaired text; NULL on failure |
| `void rwkv_pf_free_string(char *ptr)` | Frees the strings above; NULL is a no-op |

- All functions are pure and safe to call concurrently; panics are caught with
  `catch_unwind` and never cross the FFI boundary.
- Input must be valid UTF-8. The JSON functions return `{"error":…}` for NULL/invalid
  UTF-8 so wrappers can handle failures uniformly.
- Every heap string is allocated by this library and must be freed by this library
  (calling `free()` across CRTs on Windows crashes).

## Language bindings

| Language | File | Transport | Status |
|---|---|---|---|
| Python | `bindings/python/rwkv_prompt_format.py` | ctypes | ✅ tested (Python 3.12; 3.8+ compatible) |
| Node.js | `bindings/node/index.js` + `index.d.ts` | [koffi](https://koffi.dev) FFI | ✅ tested (Node 22 + koffi 3.3.3) |
| Go | `bindings/go/` | JSON CLI (`os/exec`, no cgo) | ✅ tested (Go 1.25) |
| Java | `bindings/java/ai00x/rwkv/RwkvPromptFormat.java` | JNA | ⚠️ not run yet (no JDK on the dev machine) |
| C# | `bindings/csharp/RwkvPromptFormat.cs` | P/Invoke | ⚠️ not run yet (no .NET on the dev machine) |
| Anything else | CLI | `--json` | Any language that can spawn a process |

```python
from rwkv_prompt_format import check, fix          # RWKV_PROMPT_FORMAT_LIB selects the library
report = check(text)
if not report["ok"]:
    text = fix(text)["text"]
```

```js
const pf = require('./bindings/node');             // requires pnpm add koffi
const report = pf.check(text);
const fixed = pf.fixText(text);
```

```go
import rwkvpf "github.com/ai00-x/rwkv-prompt-format/bindings/go"

report, err := rwkvpf.Check(text)                  // RWKV_PROMPT_FORMAT_CLI or PATH
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

The same Rust core is compiled to `wasm32-unknown-unknown`. It reuses the C ABI
(`rwkv_pf_check/fix/fix_text/free_string`) and adds three WASM-only exports:

| Export | Description |
|---|---|
| `rwkv_pf_wasm_alloc(len) -> ptr` | Allocate `len` zero-filled bytes for the input string |
| `rwkv_pf_wasm_free(ptr, len)` | Free the input buffer |
| `rwkv_pf_wasm_abi() -> u32` | ABI version, currently `1` |

- The module has **no imports**; it exports `memory` and the functions above. No
  wasm-bindgen required.
- The input buffer must end with NUL (`index.mjs` handles this); returned strings are
  freed with `rwkv_pf_free_string`.
- Size is 202,998 bytes, sha256
  `c0fe38e101b40986b4c99cf345b07f30ed50df24d9d0cb5bc033724b21ffbed6`. The
  `wasm-release` profile uses `opt-level="z"` + LTO + `panic="abort"` + strip.

Node:

```js
import { check, fix, fixText, FIX_ALL } from './wasm/index.mjs';

const report = await check(text);
const fixed  = await fix(text, FIX_ALL);
const text2  = await fixText(text, FIX_ALL);
```

Node loads the adjacent `.wasm` automatically. Browser/edge:

```js
import { init, check } from './wasm/index.mjs';
await init(new URL('./rwkv_prompt_format.wasm', import.meta.url));
const report = await check(text);
```

`init(source)` accepts a URL/path string, `Response`, `ArrayBuffer`, TypedArray, or
`WebAssembly.Module`. Flags: `FIX_SAFE=1`, `FIX_STRUCTURAL=2`,
`FIX_STRIP_METADATA=4`, `FIX_ALL=3`. CLI:

```bash
node wasm/cli.mjs check --stdin --json < prompt.txt
node wasm/cli.mjs fix --stdin < prompt.txt
```

Its `check` / `fix --json` output is **field-for-field identical** to the Rust CLI
(30 inputs × 4 flag combinations), including exit codes. The browser path uses standard
APIs but has not been run in a real browser; Node 22 was used to test five `init`
sources: bytes, ArrayBuffer, Response, URL, and Module.

## MCP server

`mcp/server.mjs` is a zero-dependency stdio MCP server (Node ≥ 18) that exposes the same
WASM as three tools:

| Tool | Arguments | Result |
|---|---|---|
| `check_prompt_format` | `{ text }` | `FormatReport` |
| `fix_prompt_format` | `{ text, mode?: "safe"/"structural"/"both", strip_metadata?: boolean }` | `FixResult` (including `text`) |
| `get_prompt_format_rules` | `{}` | Condensed official rules |

Resource: `rwkv-prompt-format://rules` (`text/markdown`). Tool results include both
`content[].text` (human-readable JSON) and `structuredContent` (machine-readable).
Invalid arguments return `isError: true`; an unknown tool returns JSON-RPC `-32602`.
Every tool carries `title`, `annotations` (check/rules are read-only, fix is idempotent
and non-destructive), and `outputSchema`; `initialize` also returns `instructions` so
clients can steer agents through check → fix → verify.

Configuration (the installer writes this for you):

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

- Transport is **newline-delimited JSON-RPC 2.0** (MCP stdio), not `Content-Length`
  framing; stdout is protocol-only and all logs go to stderr.
- Supported protocol versions: `2024-11-05` / `2025-03-26` / `2025-06-18`; unknown
  versions fall back to `2025-06-18`. Input limit: 8 MiB.
- Pure text, no network, no file access; stateless and safe to call concurrently.

## Agent skill and one-prompt install

`skills/rwkv-prompt-format/` is an AI-agent skill: `SKILL.md` explains when to check,
the check → fix → verify workflow, hard rules, and things not to do;
`references/rules.md` contains the full rules and finding codes. The bundled
`wasm/cli.mjs` is a fallback when MCP tools are unavailable.

`scripts/install-agent.mjs` does two things:

1. copies the skill, MCP server, and WASM into a self-contained directory (no dependency
   on this repository's path);
2. merges `mcpServers["rwkv-prompt-format"]` into the Qoder config (preserves other keys,
   backs up before writing, writes atomically; if the settings JSON is broken, it
   **writes nothing**).

| Scope | Skill installs to | MCP config |
|---|---|---|
| `user` (default) | `~/.qoder-cn/skills/rwkv-prompt-format/` | `~/.qoder-cn/settings.json` |
| `project` | `<project>/.qoder/skills/rwkv-prompt-format/` | `<project>/.qoder/settings.json` |
| `local` | `<project>/.qoder/skills/rwkv-prompt-format/` | `<project>/.qoder/settings.local.json` |

One-prompt install (paste this into Qoder or any agent):

> Run `node <path-to>/rwkv-prompt-format/scripts/install-agent.mjs --scope user --yes`,
> then remind me to run `/mcp reload` and `/skills reload`. When I work on an RWKV
> prompt, first call `check_prompt_format`, and if needed call `fix_prompt_format` with
> `mode="both"` and confirm `after.ok`.

Or run it yourself:

```bash
node scripts/install-agent.mjs --scope user --yes
node scripts/install-agent.mjs --scope user --dry-run          # print the plan only
node scripts/install-agent.mjs --scope user --uninstall --yes   # uninstall
```

Reload after installing: `/mcp reload` (MCP) and `/skills reload` (or restart the
session). Uninstall removes only the MCP entry and skill directory that this installer
created; a same-named entry pointing elsewhere is kept and reported with exit code 1.

## Limitations

- It checks **format only**. It does not judge answer quality and does not load a model
  for perplexity.
- Only the official G1/G1x templates are supported. ChatML/Llama 3/JSONL/Markdown are
  **input** formats and are converted to G1; other templates (Llama 2, Mistral, private
  chat templates) are not guessed, and an inline `<|im_start|>` is never treated as a
  structural marker.
- Rule codes are a stable API; `message` / `evidence` text may improve and should not be
  used for assertions. Human-readable output is currently in Chinese.
- Errors may remain after fixing (for example a prompt starting with `Assistant:` or a
  mix of instruction and chat); the report lists them and `notes` explains why they were
  not handled automatically.
- CLI startup takes a few milliseconds; for batch workloads use the C ABI, ctypes,
  koffi, or WASM instead of spawning a process per item.
- The WASM module is plain `wasm32-unknown-unknown`, no WASI, with no file or network
  access; the caller loads it and passes text. The browser path has not been tested in a
  real browser. `wasm-opt` could shrink it further but is not installed here.
- MCP supports stdio only (no HTTP/SSE) and needs Node ≥ 18; tool input is capped at
  8 MiB.
- C ABI/WASM inputs are NUL-terminated, so text cannot contain `\0`; the WASM wrapper and
  MCP server reject it explicitly instead of truncating.
- The installer only writes Qoder's `settings.json` / `settings.local.json` and skill
  directory. Claude Desktop, Cursor, and other clients need the MCP config copied
  manually. Reload with `/mcp reload` and `/skills reload` (or restart) after installing.

## Testing

The full validation baseline from 2026-10-21 (temporary test suites, removed after
validation):

| Suite | Size | Coverage |
|---|---|---|
| `check` matrix | 73 cases | finding code/severity/line for each format, generation point, stats cross-checks, determinism |
| `fix` exact output | 34 cases | byte-exact output for safe / structural / metadata modes, foreign template conversion |
| `fix` mode invariants | 44 cases | `changed/after/ok/applied/notes` consistency across three flag sets, idempotency |
| Deterministic fuzzing | 6000 cases | 1500 random label/colon/whitespace/fence/foreign-template inputs per flag set; `fix(fix(x)) == fix(x)`, `after == check(text)`, residual errors always have notes, no CR/BOM in output |
| CLI | 27 checks | exit codes 0/1/2, three input sources, `--json`, `--quiet`, `--in-place`, invalid UTF-8, Unicode and 1 MB round-trips |
| C ABI | 1 suite | JSON identical to the CLI, 8-thread concurrency, 3000 calls without memory growth, NULL/invalid UTF-8, static version pointer, 1 MB input |
| Bindings | 3 suites | Python, Node (koffi), and Go parity for check/fix/idempotency/Unicode/large inputs |
| WASM (incl. wrapper) | 304 checks | 30 inputs × 4 flag sets field-for-field identical to the Rust CLI, matching exit codes, idempotency; five `init` sources; 1 MB; 100 concurrent calls; 5000 alloc/free cycles without growth; no imports |
| MCP | 27 checks | initialize/protocol versions, tools/list, all three tools, resources, error codes, structured results, no replies to notifications, batch rejection, clean stdout, 9 KB input |
| Skill installer | 48 checks | user/project/local scopes, settings merge preserving other keys, idempotent re-run, broken JSON writes nothing, uninstall ownership, installed MCP server actually runs |

The one-off validation totals **379 checks, 0 failures**. Java/C# bindings were not run
(no JDK/.NET available); the browser WASM path was not run in a real browser.

The repeatable critical paths are checked in as `scripts/verify.mjs` (**74 checks**,
34 with `--quick`). It uses no temporary files and writes nothing to the repository:

| Suite | Coverage |
|---|---|
| Rust ↔ WASM parity | 15 corpus inputs (chat/messy/ChatML/Llama 3/JSONL/Markdown/material QA/think/fence/BOM+CRLF/continuation/metadata/empty/plain/64 KB Unicode) × check and safe/structural/both fix; `changed`, `after == check(text)`, residual errors have notes, idempotency |
| MCP | initialize/instructions, tools/list (title/annotations/outputSchema), check/fix, resources, unknown method/tool, invalid mode, NUL rejection, no reply to notifications, ping |
| Installer | user-scope install, merge preserving unrelated keys, idempotent re-run, installed MCP server actually runs, broken JSON writes nothing, foreign entry preserved, uninstall cleanup |
| Metadata | `--strip-metadata` parity between the Rust and WASM stacks |

```bash
cargo fmt --all -- --check && cargo clippy --all-targets -- -D warnings
cargo build --release
node scripts/build-wasm.mjs --check
node scripts/verify.mjs
printf 'User: hi\n\n\nAssistant:' | ./target/release/rwkv-pf fix --stdin
node wasm/cli.mjs check --text "User: hi" --json
```

`verify.mjs` needs `target/release/rwkv-pf` and the committed WASM. A CI job can run
these commands in order; no workflow file is included yet.

## Contributing

Issues and pull requests are welcome. Before opening a PR, run `cargo fmt --all`,
`cargo clippy --all-targets -- -D warnings`, `node scripts/build-wasm.mjs --check`, and
`node scripts/verify.mjs`. Rule codes are a public API — do not rename them casually.

## License

Dual-licensed under **MIT OR Apache-2.0**. See [LICENSE-MIT](LICENSE-MIT) and
[LICENSE-APACHE](LICENSE-APACHE).
