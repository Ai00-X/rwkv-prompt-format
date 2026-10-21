# RWKV G1/G1x prompt format — reference

Source of truth: RWKV's official G1/G1x templates
(`BlinkDL/RWKV-LM · RWKV-v7/RWKV7-G1x-templates.txt`) and rwkv.cn's format guide.
This checker validates **syntax and structure**, not factual correctness.

## 1. Block structure

A prompt is a sequence of labelled blocks. Blocks are separated by **exactly one
empty line** (`\n\n`).

```
System: You are a careful assistant.

User: What is the capital of France?

Assistant:
```

Rules:

- One blank line between blocks, no more, no less.
- No blank line inside a block's content. If content needs a paragraph break,
  that is a sign the prompt should be split into blocks or rewritten.
- No leading or trailing blank lines.
- A block's content may start on the label line (`User: hi`) or on the next line
  (`User:\n材料：…`) — both are official; the layout is preserved by the fixer.

## 2. Labels

Case-sensitive, each followed by exactly one ASCII space:

| Label | Role |
|---|---|
| `System:` | system prompt / persona / tools |
| `User:` | user turn |
| `Assistant:` | model turn or generation point |
| `Instruction:` | task instruction (instruction mode) |
| `Input:` | task input (instruction mode) |
| `Response:` | model answer or generation point (instruction mode) |

Common errors:

```
user: hi          → LABEL_CASE        → User: hi
User : hi         → LABEL_SPACE_BEFORE_COLON
User：hi          → LABEL_FULLWIDTH_COLON
User:hi           → LABEL_NO_SPACE
  User: hi        → LABEL_INDENTED
User:  hi         → LABEL_EXTRA_SPACE
```

## 3. Generation point

The prompt must end at a generation point so the model knows where to continue:

- chat: `Assistant:`
- instruction: `Response:`

It must be the last block, with nothing after it. A deliberate seed is allowed:
`Assistant: {"answer": ` — including the trailing space.

Other common end states:

| Finding | Meaning |
|---|---|
| `ENDS_ON_USER` | prompt ends on `User:`; add `Assistant:` |
| `ENDS_ON_NON_ASSISTANT` | ends on a system/instruction/input block |
| `MISSING_GENERATION_POINT` | no trailing assistant/response |
| `GEN_PREFIX_TRAILING_SPACE` | `Assistant: ` with a stray space at the very end |
| `SEEDED_GENERATION_POINT` | generation point has seed content (info, often valid) |

## 4. Chat vs instruction

Chat:

```
System: ...

User: ...

Assistant:
```

Instruction:

```
System: ...

Instruction: ...

Input: ...

Response:
```

Do not mix `Instruction:`/`Input:`/`Response:` with `User:`/`Assistant:` in the
same prompt (`MIXED_INSTRUCTION_CHAT`). Instruction blocks are ordered
`Instruction → Input → Response` (`INSTRUCTION_ORDER`).

## 5. Think mode

The think prefix is lowercase and verbatim. Allowed forms after `Assistant:`:

```
Assistant: <think
Assistant: <think></think
Assistant: <think>
</think
```

Hints go at the **end of the user text**, lowercase:

```
User: ... (think)
User: ... (think a bit)
User: ... (think a lot)
```

Wrong: `(Think)`, `（think）`, `( think )`, multiple hints, hint not at the end.

## 6. Tools / function calling

`System: Tools:` is a literal, case-sensitive prefix. Put it at the start of the
system block, then the tool schemas:

```
System: Tools:
[{"type":"function","function":{...}}]

User: ...

Assistant:
```

- `Tools:` is not a role label and must not start its own block.
- `tools:` / `TOOLS:` / `Tools：` are rejected.

## 7. Material QA

The official material question-answer layout puts content on the next line:

```
User:
材料：...（资料正文）
问题：...（问题）

Assistant: {"answer": 
```

- `材料：` and `问题：` use full-width colons and are part of the User content.
- The seed `{"answer": ` after `Assistant:` is intentional; do not trim its
  trailing space.

## 8. Continuation

A prompt may end with a language continuation label instead of an empty
generation point:

```
User: Translate to English: ...

English:
```

Supported labels include `English:`, `Chinese:`, `Japanese:`, `Korean:`,
`French:`, `German:`, `Spanish:`, `Russian:`, `Portuguese:`, `Italian:`,
`Arabic:`, `Hindi:`. They are case-sensitive.

## 9. Code fences

Fenced content is literal. A label line may open a fence:

```
Assistant: ```json
{"ok": true}
```
```

Role-like lines inside a fence are not block boundaries. They are reported as
`ROLE_IN_CODE_FENCE` (warn) if they look like a role label; an unclosed fence is
`UNCLOSED_CODE_FENCE`.

## 10. What the fixer changes

Safe (`mode: "safe"`):

- BOM, CRLF/CR → LF;
- label case, indentation, extra spaces, full-width colons, spaces before colons;
- block separator count, blank lines inside content, leading/trailing blank lines;
- think prefix and hint normalization;
- `Tools:` spelling/placement;
- unclosed code fences.

Structural (`mode: "structural"`):

- ChatML (`<|im_start|>…<|im_end|>`), Llama 3 headers, JSONL messages, Markdown
  roles, `Human:`/`AI:`/`Question:`/`Answer:` → G1 blocks;
- merge multiple `System:` blocks (System first);
- merge adjacent same-role blocks;
- reorder instruction blocks;
- add the generation point;
- wrap unlabelled plain text as `User:` or a preamble `System:` block.

Not changed: semantic content. `strip_metadata` is the only content edit and is
off by default. Unsupported templates (Llama 2 `[INST]`, Mistral, private chat
templates) are not guessed; the fix result includes `notes`.

## 11. Stable finding codes (selected)

Use `findings[].code` for automation; `message` and `evidence` may change.

| Code | Severity | Meaning |
|---|---|---|
| `EMPTY_PROMPT` | error | no usable text |
| `NO_ROLE_LABEL` | error | plain text with no labels |
| `EXTRA_BLANK_SEPARATOR` | error | more than one blank line between blocks |
| `MISSING_BLANK_SEPARATOR` | error | two labels without a blank line |
| `BLANK_LINE_IN_CONTENT` | error | empty line inside a block |
| `LEADING_BLANK_LINES` / `TRAILING_BLANK_LINES` | warn/error | stray blank lines |
| `LABEL_CASE` | error | wrong case for a label |
| `LABEL_FULLWIDTH_COLON` | error | `：` instead of `:` |
| `LABEL_NO_SPACE` / `LABEL_EXTRA_SPACE` | error/warn | spacing after a label |
| `MULTIPLE_SYSTEM` | error | more than one `System:` block |
| `SYSTEM_NOT_FIRST` | error | system block not at the top |
| `MIXED_INSTRUCTION_CHAT` | error | instruction and chat blocks mixed |
| `INSTRUCTION_ORDER` | error | instruction blocks out of order |
| `THINK_MALFORMED` / `THINK_PREFIX_CASE` | error | invalid think prefix |
| `THINK_HINT_NOT_AT_END` / `THINK_HINT_CASE` | error | invalid think hint |
| `TOOLS_LABEL_CASE` / `TOOLS_COLON` | error | invalid `Tools:` label |
| `ROLE_IN_CODE_FENCE` | warn | role-like line inside a fence |
| `UNCLOSED_CODE_FENCE` | error | fence opened but not closed |
| `FOREIGN_MARKER` | error | ChatML/Llama/JSONL/Markdown marker left in text |
| `MEM_META` | info | leading `[seg …]` metadata prefix |

## 12. Examples

Bad:

```
user: hi


Assistant : 
```

Fixed:

```
User: hi

Assistant:
```

Bad (converted from ChatML):

```
<|im_start|>system
You are helpful.<|im_end|>
<|im_start|>user
hi<|im_end|>
<|im_start|>assistant
```

Fixed:

```
System: You are helpful.

User: hi

Assistant:
```
