---
name: rwkv-prompt-format
description: Check and repair prompts for RWKV G1/G1x models. Use whenever writing, converting, debugging, or sending a prompt to an RWKV checkpoint, when RWKV output degrades or format drift is suspected, or when text must follow the exact System/User/Assistant/Instruction/Input/Response layout. Provides MCP tools check_prompt_format, fix_prompt_format, get_prompt_format_rules, plus a zero-dependency WASM CLI fallback.
---

# RWKV prompt format

RWKV G1/G1x is stricter about prompt syntax than Transformer chat models. A missing
blank line, a capitalized label, or a stray space at the generation point can degrade
output without any API error. Check the **exact bytes you are about to send**, not a
paraphrase.

## Workflow

1. **Get the exact text.** Use the prompt as the user supplied it. Do not normalize it
   from memory or reconstruct it from the conversation.
2. **Check first.** Call `check_prompt_format` with `{ "text": "<exact prompt>" }`.
   - `ok: true` → use the text unchanged.
   - `ok: false` → continue.
3. **Fix.** Call `fix_prompt_format` with `{ "text": "<exact prompt>", "mode": "both" }`.
   Use `structuredContent.text` as the repaired prompt. Check `structuredContent.after.ok`.
4. **Verify.** If `after.ok` is false, read `after.findings` and `notes`. Report the
   residual errors. Do not invent a fix that changes meaning; only apply safe,
   format-only corrections and re-check.
5. **Report briefly.** State what was changed (`before`/`after` counts, `applied` codes)
   and any residual errors. Then use or return the repaired text.

If the user only asks for rules or an explanation, call `get_prompt_format_rules`
or read `references/rules.md`. Do not modify a prompt that was not going to be sent.

## Fix modes

| `mode` | Changes |
|---|---|
| `safe` | BOM/CRLF, label case/spacing/colons, blank lines, think prefixes/hints, `Tools:` label, fence closure |
| `structural` | Convert ChatML / Llama 3 / JSONL / Markdown / Human-AI to G1, merge blocks, reorder Instruction → Input → Response, add generation point |
| `both` | Default. Safe + structural |

`strip_metadata: true` removes leading `[seg 12 · user]` memory prefixes. This is the
only content edit; leave it `false` unless the user explicitly asks.

The fixer is deterministic and idempotent: fixing the returned text again yields the
same text. Do not loop it.

## Hard rules (short version)

- Blocks are separated by **exactly one blank line** (`\n\n`). Never put a blank line
  inside block content.
- Labels are **case-sensitive** and followed by exactly one ASCII space:
  `System:`, `User:`, `Assistant:`, `Instruction:`, `Input:`, `Response:`.
- Chat: `System: ...\n\nUser: ...\n\nAssistant:`.
- Instruction: `System: ...\n\nInstruction: ...\n\nInput: ...\n\nResponse:`.
- The last line is the generation point: `Assistant:` for chat, `Response:` for
  instruction, with nothing after it (or a deliberate seed such as `{"answer": `).
- If a `System:` block exists, it comes first; merge multiple `System:` blocks.
- Think: `Assistant: <think`, `Assistant: <think></think`, or
  `Assistant: <think>\n</think` — lowercase, verbatim. Hints `(think)`,
  `(think a bit)`, `(think a lot)` go at the end of the user text.
- Function calling: `System: Tools:` is a literal case-sensitive prefix.
- Material QA: `User:\n材料：...\n问题：...\n\nAssistant: {"answer": ` — the content
  starts on the next line, and the seed space after `{"answer":` is significant.
- Fenced code is literal: role-like lines inside a fence are not block boundaries.
- Do not wrap the whole prompt in a Markdown code fence when sending it to the model.

Full rules with examples: `references/rules.md`.

## Fallback when MCP tools are unavailable

The skill ships a zero-dependency WASM CLI. Use it only when
`check_prompt_format` / `fix_prompt_format` are not in the tool list.
`{{SKILL_DIR}}` is replaced with this skill's absolute path by the installer
(in the repository source, substitute the `skills/rwkv-prompt-format` path):

```bash
node "{{SKILL_DIR}}/wasm/cli.mjs" check --stdin --json < prompt.txt
node "{{SKILL_DIR}}/wasm/cli.mjs" fix   --stdin --json < prompt.txt
node "{{SKILL_DIR}}/wasm/cli.mjs" fix   --stdin --safe-only < prompt.txt
```

- Exit code `0` = success; `1` = errors remain (or would remain); `2` = usage/IO error.
- `--json` gives the same `FormatReport` / `FixResult` schema as the MCP tools.
- `fix` writes the repaired text to stdout and the summary to stderr unless `--json`.

## Do not

- Do not silently rewrite user content, translate, summarize, or "improve" wording.
- Do not add or remove turns to make a conversation look nicer.
- Do not use `strip_metadata` unless asked.
- Do not claim the prompt is valid without running the checker, unless the MCP tools
  and the CLI are both unavailable — then say so explicitly.
- Do not treat `message` / `evidence` text as stable API; use `findings[].code`.

## Reference

- `references/rules.md` — full rule set, examples, and stable finding codes.
- `get_prompt_format_rules` — the same condensed rules from the MCP server.
