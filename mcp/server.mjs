#!/usr/bin/env node
/**
 * RWKV G1/G1x prompt format MCP server (stdio, zero dependencies).
 *
 * Transport: newline-delimited JSON-RPC 2.0 on stdin/stdout (MCP stdio).
 * All diagnostics go to stderr so stdout stays a clean protocol stream.
 *
 * Tools:
 *   - check_prompt_format
 *   - fix_prompt_format
 *   - get_prompt_format_rules
 *
 * The checker itself is the checked-in WASM artifact; this server never
 * touches the network and never reads user files.
 */

import { existsSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { createInterface } from 'node:readline';
import { fileURLToPath, pathToFileURL } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const WRAPPER = [
  join(HERE, 'assets', 'index.mjs'),
  join(HERE, '..', 'wasm', 'index.mjs'),
].find(existsSync);

if (!WRAPPER) {
  console.error('rwkv-prompt-format-mcp: cannot find wasm wrapper (assets/index.mjs or ../wasm/index.mjs)');
  process.exit(1);
}

const { FIX_SAFE, FIX_STRUCTURAL, FIX_STRIP_METADATA, check, fix, init, version } = await import(
  pathToFileURL(WRAPPER).href
);

const SERVER_NAME = 'rwkv-prompt-format';
const MAX_TEXT_BYTES = 8 * 1024 * 1024;
const SUPPORTED_PROTOCOLS = ['2024-11-05', '2025-03-26', '2025-06-18'];

const RULES = `# RWKV G1/G1x prompt format (official, condensed)

Use this before writing or repairing a prompt for an RWKV G1/G1x checkpoint.

## Hard rules
- Blocks are separated by EXACTLY one blank line (\`\\n\\n\`). Never put a blank line inside a block.
- Labels are case-sensitive: \`System:\`, \`User:\`, \`Assistant:\`, \`Instruction:\`, \`Input:\`, \`Response:\`.
- A label is followed by exactly one ASCII space, then content. The final generation point is
  \`Assistant:\` (chat) or \`Response:\` (instruction) with nothing after it; a seed may follow a space.
- Chat: \`System: ...\\n\\nUser: ...\\n\\nAssistant:\`.
- Instruction: \`System: ...\\n\\nInstruction: ...\\n\\nInput: ...\\n\\nResponse:\`.
- If a System block exists it comes first; multiple System blocks must be merged.
- Material QA: \`User:\\n材料：...\\n问题：...\\n\\nAssistant: {"answer": \` (content starts on the next line).
- Think mode prefixes (verbatim): \`Assistant: <think\`, \`Assistant: <think></think\`, or
  \`Assistant: <think>\\n</think\`. Lowercase only.
- Think-length hints go at the END of the user text: \`(think)\`, \`(think a bit)\`, \`(think a lot)\`,
  with one ASCII space before the parenthesis. Lowercase only.
- Function calling: \`System: Tools:\` is a literal, case-sensitive prefix; tool schemas follow.
- Continuation: the prompt may end with \`English:\`, \`Chinese:\`, etc. (12 language names).
- Do not put role labels inside code fences; fenced content is literal.

## What the fixer may change
- Safe: label spelling/case/indent/colons/spacing, blank lines, BOM/CRLF, think prefix/hints, Tools label, unclosed fences.
- Structural: convert ChatML / Llama 3 / JSONL / Markdown / Human-AI into G1 plain, merge blocks,
  reorder Instruction -> Input -> Response, add a generation point.
- Semantic content is NOT rewritten. \`strip_metadata\` is the only content edit and is off by default.

## Recommended workflow
1. \`check_prompt_format\` on the exact text you are about to send.
2. If \`ok\` is false, call \`fix_prompt_format\` with \`mode: "both"\`.
3. Use the returned \`text\`; verify \`after.ok\` is true. If residual errors remain, read \`after.findings\`
   and \`notes\`; do not invent fixes that change meaning.
4. Re-run \`check_prompt_format\` on the final text if you edited it manually.`;

// Output schemas are intentionally additive: no additionalProperties:false, so
// new report fields never break clients that validate structuredContent.
const FINDING_SCHEMA = {
  type: 'object',
  properties: {
    code: { type: 'string', description: 'Stable rule code, e.g. EXTRA_BLANK_SEPARATOR.' },
    severity: { type: 'string', enum: ['error', 'warn', 'info'] },
    line: { type: 'integer', minimum: 0, description: '1-based line; 0 means whole-text.' },
    message: { type: 'string' },
    evidence: { type: 'string' },
  },
  required: ['code', 'severity', 'line', 'message', 'evidence'],
};

const STATS_SCHEMA = {
  type: 'object',
  properties: {
    chars: { type: 'integer' },
    lines: { type: 'integer' },
    user_blocks: { type: 'integer' },
    assistant_blocks: { type: 'integer' },
    system_blocks: { type: 'integer' },
    instruction_blocks: { type: 'integer' },
    input_blocks: { type: 'integer' },
    response_blocks: { type: 'integer' },
    blank_separators: { type: 'integer' },
    max_blank_run: { type: 'integer' },
    metadata_lines: { type: 'integer' },
    think_prefix: { type: 'boolean' },
    function_call: { type: 'boolean' },
  },
  required: [
    'chars', 'lines', 'user_blocks', 'assistant_blocks', 'system_blocks',
    'instruction_blocks', 'input_blocks', 'response_blocks', 'blank_separators',
    'max_blank_run', 'metadata_lines', 'think_prefix', 'function_call',
  ],
};

const REPORT_SCHEMA = {
  type: 'object',
  properties: {
    profile: { type: 'string' },
    format: {
      type: 'string',
      enum: [
        'plain_chat', 'material_qa', 'instruction', 'instruction_chat', 'system_tools',
        'chatml', 'markdown', 'jsonl', 'continuation', 'unknown',
      ],
    },
    ok: { type: 'boolean', description: 'True when error_count is 0.' },
    error_count: { type: 'integer' },
    warning_count: { type: 'integer' },
    info_count: { type: 'integer' },
    generation_point: {
      type: ['object', 'null'],
      properties: {
        line: { type: 'integer' },
        role: { type: 'string' },
        seeded: { type: 'boolean' },
      },
      required: ['line', 'role', 'seeded'],
    },
    stats: STATS_SCHEMA,
    findings: { type: 'array', items: FINDING_SCHEMA },
  },
  required: [
    'profile', 'format', 'ok', 'error_count', 'warning_count', 'info_count',
    'generation_point', 'stats', 'findings',
  ],
};

const FIX_RESULT_SCHEMA = {
  type: 'object',
  properties: {
    changed: { type: 'boolean' },
    options: {
      type: 'object',
      properties: {
        safe: { type: 'boolean' },
        structural: { type: 'boolean' },
        strip_metadata: { type: 'boolean' },
      },
      required: ['safe', 'structural', 'strip_metadata'],
    },
    applied: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          code: { type: 'string' },
          count: { type: 'integer' },
          message: { type: 'string' },
        },
        required: ['code', 'count', 'message'],
      },
    },
    notes: { type: 'array', items: { type: 'string' } },
    before: REPORT_SCHEMA,
    after: REPORT_SCHEMA,
    text: { type: 'string', description: 'The fixed prompt.' },
  },
  required: ['changed', 'options', 'applied', 'notes', 'before', 'after', 'text'],
};

const TOOLS = [
  {
    name: 'check_prompt_format',
    title: 'Check RWKV prompt format',
    description:
      'Check whether a prompt follows the official RWKV G1/G1x format (block separators, case-sensitive labels, ' +
      'generation point, think/Tools syntax, continuation labels). Use before sending a prompt to an RWKV model, ' +
      'when RWKV output degrades, or when converting a prompt from ChatML/Llama/JSON. Pure text; no model or network.',
    inputSchema: {
      type: 'object',
      properties: {
        text: {
          type: 'string',
          description: 'The exact prompt text to check (UTF-8, max 8 MiB).',
        },
      },
      required: ['text'],
      additionalProperties: false,
    },
    outputSchema: REPORT_SCHEMA,
    annotations: {
      readOnlyHint: true,
      destructiveHint: false,
      idempotentHint: true,
      openWorldHint: false,
    },
  },
  {
    name: 'fix_prompt_format',
    title: 'Fix RWKV prompt format',
    description:
      'Deterministically repair a prompt into RWKV G1/G1x format and return the fixed text plus before/after ' +
      'reports. Safe mode only normalizes syntax/whitespace; structural mode also converts ChatML/Llama 3/JSONL/' +
      'Markdown and merges/adds blocks. It does not rewrite semantic content unless strip_metadata is true. ' +
      'The call is idempotent: fixing the returned text again produces the same text.',
    inputSchema: {
      type: 'object',
      properties: {
        text: {
          type: 'string',
          description: 'The prompt to repair (UTF-8, max 8 MiB).',
        },
        mode: {
          type: 'string',
          enum: ['safe', 'structural', 'both'],
          default: 'both',
          description: 'safe = syntax/whitespace only; structural = convert/merge/reorder; both = default.',
        },
        strip_metadata: {
          type: 'boolean',
          default: false,
          description: 'Drop leading "[seg 12 · user]" memory-metadata prefixes. This edits content; off by default.',
        },
      },
      required: ['text'],
      additionalProperties: false,
    },
    outputSchema: FIX_RESULT_SCHEMA,
    annotations: {
      readOnlyHint: false,
      destructiveHint: false,
      idempotentHint: true,
      openWorldHint: false,
    },
  },
  {
    name: 'get_prompt_format_rules',
    title: 'Get RWKV prompt format rules',
    description:
      'Return a condensed, authoritative cheat sheet for RWKV G1/G1x prompt format: block separators, labels, ' +
      'generation points, think mode, Tools/function calling, continuation, and what the fixer will and will not change.',
    inputSchema: {
      type: 'object',
      properties: {},
      additionalProperties: false,
    },
    outputSchema: {
      type: 'object',
      properties: { rules: { type: 'string', description: 'Markdown cheat sheet.' } },
      required: ['rules'],
    },
    annotations: {
      readOnlyHint: true,
      destructiveHint: false,
      idempotentHint: true,
      openWorldHint: false,
    },
  },
];

const RESOURCES = [
  {
    uri: 'rwkv-prompt-format://rules',
    name: 'RWKV G1/G1x prompt format rules',
    description: 'Condensed official format rules and the recommended check -> fix -> verify workflow.',
    mimeType: 'text/markdown',
  },
];

class ToolInputError extends Error {}

class RpcError extends Error {
  constructor(code, message) {
    super(message);
    this.code = code;
  }
}

function send(message) {
  process.stdout.write(`${JSON.stringify(message)}\n`);
}

function result(id, value) {
  send({ jsonrpc: '2.0', id, result: value });
}

function rpcError(id, code, message, data) {
  const error = { code, message };
  if (data !== undefined) error.data = data;
  send({ jsonrpc: '2.0', id, error });
}

function toolResult(value, text = JSON.stringify(value, null, 2)) {
  return {
    content: [{ type: 'text', text }],
    structuredContent: value,
    isError: false,
  };
}

function toolError(message) {
  return {
    content: [{ type: 'text', text: message }],
    isError: true,
  };
}

function requireText(args) {
  if (!args || typeof args.text !== 'string') {
    throw new ToolInputError('"text" is required and must be a string');
  }
  if (args.text.includes('\0')) {
    throw new ToolInputError('"text" must not contain NUL characters (the C/WASM ABI is NUL-terminated)');
  }
  const bytes = new TextEncoder().encode(args.text).length;
  if (bytes > MAX_TEXT_BYTES) {
    throw new ToolInputError(`"text" is ${bytes} bytes; the limit is ${MAX_TEXT_BYTES} bytes`);
  }
  return args.text;
}

function fixFlags(args) {
  const mode = args.mode ?? 'both';
  let flags;
  switch (mode) {
    case 'safe':
      flags = FIX_SAFE;
      break;
    case 'structural':
      flags = FIX_STRUCTURAL;
      break;
    case 'both':
      flags = FIX_SAFE | FIX_STRUCTURAL;
      break;
    default:
      throw new ToolInputError('"mode" must be one of: safe, structural, both');
  }
  if (args.strip_metadata !== undefined && typeof args.strip_metadata !== 'boolean') {
    throw new ToolInputError('"strip_metadata" must be a boolean');
  }
  if (args.strip_metadata) flags |= FIX_STRIP_METADATA;
  return flags;
}

async function callTool(params) {
  if (!params || typeof params.name !== 'string') {
    throw new ToolInputError('"name" is required');
  }
  const args = params.arguments ?? {};
  if (typeof args !== 'object' || Array.isArray(args)) {
    throw new ToolInputError('"arguments" must be an object');
  }
  switch (params.name) {
    case 'check_prompt_format':
      return toolResult(await check(requireText(args)));
    case 'fix_prompt_format':
      return toolResult(await fix(requireText(args), fixFlags(args)));
    case 'get_prompt_format_rules':
      return toolResult({ rules: RULES }, RULES);
    default:
      throw new RpcError(-32602, `unknown tool: ${params.name}`);
  }
}

async function handle(message) {
  if (Array.isArray(message)) {
    rpcError(null, -32600, 'batch requests are not supported');
    return;
  }
  if (!message || typeof message !== 'object') {
    rpcError(null, -32600, 'invalid request');
    return;
  }
  const hasId = Object.prototype.hasOwnProperty.call(message, 'id');
  const id = hasId ? message.id : null;
  if (message.jsonrpc !== '2.0' || typeof message.method !== 'string') {
    if (hasId) rpcError(id, -32600, 'invalid JSON-RPC 2.0 request');
    return;
  }
  const params = message.params ?? {};
  // Never reply to notifications; only notifications/* are accepted as such.
  if (!hasId && !message.method.startsWith('notifications/')) return;

  switch (message.method) {
    case 'initialize': {
      if (!hasId) return;
      const requested = params.protocolVersion;
      const protocolVersion = SUPPORTED_PROTOCOLS.includes(requested) ? requested : '2025-06-18';
      result(id, {
        protocolVersion,
        capabilities: {
          tools: { listChanged: false },
          resources: { subscribe: false, listChanged: false },
        },
        serverInfo: { name: SERVER_NAME, version: await version() },
        instructions:
          'RWKV G1/G1x prompt format checker/fixer. Before sending a prompt to RWKV, call ' +
          'check_prompt_format. If it is not ok, call fix_prompt_format with mode="both" and use the returned ' +
          'text after verifying after.ok. Never invent format changes that alter meaning.',
      });
      return;
    }
    case 'notifications/initialized':
    case 'notifications/cancelled':
      return;
    case 'ping':
      if (hasId) result(id, {});
      return;
    case 'tools/list':
      result(id, { tools: TOOLS });
      return;
    case 'tools/call': {
      if (!hasId) return;
      try {
        result(id, await callTool(params));
      } catch (err) {
        if (err instanceof RpcError) {
          rpcError(id, err.code, err.message);
        } else if (err instanceof ToolInputError) {
          result(id, toolError(err.message));
        } else {
          result(id, toolError(`internal error: ${err instanceof Error ? err.message : String(err)}`));
        }
      }
      return;
    }
    case 'resources/list':
      result(id, { resources: RESOURCES });
      return;
    case 'resources/read': {
      if (params.uri !== 'rwkv-prompt-format://rules') {
        rpcError(id, -32602, `unknown resource: ${params.uri}`);
        return;
      }
      result(id, {
        contents: [{ uri: RESOURCES[0].uri, mimeType: 'text/markdown', text: RULES }],
      });
      return;
    }
    default:
      if (hasId) rpcError(id, -32601, `method not found: ${message.method}`);
  }
}

try {
  await init();
} catch (err) {
  console.error(`rwkv-prompt-format-mcp: failed to load wasm: ${err instanceof Error ? err.message : err}`);
  process.exit(1);
}

const input = createInterface({ input: process.stdin, crlfDelay: Infinity, terminal: false });
let queue = Promise.resolve();

input.on('line', (line) => {
  if (!line.trim()) return;
  queue = queue.then(async () => {
    let message;
    try {
      message = JSON.parse(line);
    } catch {
      rpcError(null, -32700, 'parse error');
      return;
    }
    try {
      await handle(message);
    } catch (err) {
      console.error(`rwkv-prompt-format-mcp: ${err instanceof Error ? err.stack : err}`);
      const id = message && Object.prototype.hasOwnProperty.call(message, 'id') ? message.id : null;
      rpcError(id, -32603, 'internal error');
    }
  });
});

input.on('close', () => {
  queue.finally(() => {
    process.exitCode = 0;
  });
});
