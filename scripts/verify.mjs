#!/usr/bin/env node
/**
 * Permanent regression gate for rwkv-prompt-format.
 *
 * Runs three independent suites against the checked-in artifacts:
 *   1. Rust CLI <-> WASM CLI parity (check/fix, all modes) + fixer invariants
 *   2. MCP protocol handshake, tool surface, success and error paths
 *   3. Installer round-trip in a throwaway Qoder home (install/idempotent/uninstall/bad JSON)
 *
 * Usage:
 *   node scripts/verify.mjs           # full corpus
 *   node scripts/verify.mjs --quick   # smaller corpus, same gates
 *
 * Prerequisites:
 *   cargo build --release
 *   node scripts/build-wasm.mjs --check
 *
 * Exit codes: 0 all checks passed, 1 failure, 2 prerequisites missing.
 */

import { spawn, spawnSync } from 'node:child_process';
import { deepStrictEqual, strictEqual } from 'node:assert';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const QUICK = process.argv.includes('--quick');
const RUST_CLI = join(ROOT, 'target', 'release', process.platform === 'win32' ? 'rwkv-pf.exe' : 'rwkv-pf');
const WASM_CLI = join(ROOT, 'wasm', 'cli.mjs');
const WASM_FILE = join(ROOT, 'wasm', 'rwkv_prompt_format.wasm');
const MCP_SERVER = join(ROOT, 'mcp', 'server.mjs');
const INSTALLER = join(ROOT, 'scripts', 'install-agent.mjs');

let passed = 0;
const failures = [];

function gate(name, fn) {
  try {
    fn();
    passed += 1;
  } catch (err) {
    failures.push({ name, error: err });
  }
}

async function gateAsync(name, fn) {
  try {
    await fn();
    passed += 1;
  } catch (err) {
    failures.push({ name, error: err });
  }
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function runCli(which, args, input) {
  const command = which === 'rust' ? RUST_CLI : process.execPath;
  const argv = which === 'rust' ? args : [WASM_CLI, ...args];
  const result = spawnSync(command, argv, {
    input,
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
    windowsHide: true,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`${which} ${args.join(' ')} exited ${result.status}: ${result.stderr.slice(0, 500)}`);
  }
  try {
    return JSON.parse(result.stdout);
  } catch {
    throw new Error(`${which} ${args.join(' ')} did not emit JSON: ${result.stdout.slice(0, 300)}`);
  }
}

const CHECK_ARGS = ['check', '--stdin', '--json', '--no-fail'];

function fixArgs(mode, extra = []) {
  const flags = mode === 'safe' ? ['--safe-only'] : mode === 'structural' ? ['--structural-only'] : [];
  return ['fix', '--stdin', '--json', '--quiet', '--no-fail', ...flags, ...extra];
}

const CORPUS = [
  ['chat-ok', 'User: hi\n\nAssistant:'],
  ['messy-labels', 'user : hi\n\n\nassistant : '],
  [
    'chatml',
    '<|im_start|>system\nYou are helpful.<|im_end|>\n' +
      '<|im_start|>user\nHello<|im_end|>\n<|im_start|>assistant\n',
  ],
  [
    'llama3',
    '<|start_header_id|>system<|end_header_id|>\n\nBe nice<|eot_id|>' +
      '<|start_header_id|>user<|end_header_id|>\n\nHi<|eot_id|>' +
      '<|start_header_id|>assistant<|end_header_id|>\n\n',
  ],
  ['jsonl', '{"role":"system","content":"Be nice"}\n{"role":"user","content":"Hi"}\n'],
  ['markdown', '## System\nBe nice\n\n## User\nHi\n'],
  ['material-qa', 'User:\n材料：天空是蓝的。\n问题：为什么？\n\nAssistant: {"answer": '],
  ['think', 'User: 解释一下 (Think a Lot)\n\nAssistant: <THINK>'],
  ['unclosed-fence', 'User: 看代码\n```rust\nfn main() {}\n\nAssistant:'],
  ['bom-crlf', '\uFEFFUser: hi\r\n\r\nAssistant:'],
  ['continuation', 'User: 翻译\n\nAssistant: 你好\n\nEnglish:'],
  ['metadata', '[seg 12 · user]\nUser: hi\n\nAssistant:'],
  ['empty', ''],
  ['plain-text', 'hello world'],
  ['unicode-large', `User: ${'中文🙂'.repeat(4096)}\n\nAssistant:`],
];

const QUICK_NAMES = new Set(['chat-ok', 'messy-labels', 'chatml', 'material-qa', 'bom-crlf']);
const corpus = QUICK ? CORPUS.filter(([name]) => QUICK_NAMES.has(name)) : CORPUS;

function paritySuite() {
  for (const [name, text] of corpus) {
    gate(`parity/check/${name}`, () => {
      const rust = runCli('rust', CHECK_ARGS, text);
      const wasm = runCli('wasm', CHECK_ARGS, text);
      deepStrictEqual(wasm, rust, `check report differs for ${name}`);
    });

    for (const mode of ['safe', 'structural', 'both']) {
      gate(`parity/fix-${mode}/${name}`, () => {
        const args = fixArgs(mode);
        const rust = runCli('rust', args, text);
        const wasm = runCli('wasm', args, text);
        deepStrictEqual(wasm, rust, `fix(${mode}) result differs for ${name}`);

        const label = `${name}/${mode}`;
        strictEqual(wasm.changed, wasm.text !== text, `changed flag disagrees with text for ${label}`);
        if (!wasm.after.ok) {
          assert(wasm.notes.length > 0, `residual errors without notes for ${label}`);
        }
        const again = runCli('wasm', args, wasm.text);
        strictEqual(again.text, wasm.text, `fix is not idempotent for ${label}`);
        if (mode === 'both') {
          deepStrictEqual(
            wasm.after,
            runCli('wasm', CHECK_ARGS, wasm.text),
            `after report disagrees with check(text) for ${label}`,
          );
        }
      });
    }
  }

  gate('fix/strip-metadata', () => {
    const text = '[seg 12 · user]\nUser: hi\n\nAssistant:';
    const args = fixArgs('both', ['--strip-metadata']);
    const rust = runCli('rust', args, text);
    const wasm = runCli('wasm', args, text);
    deepStrictEqual(wasm, rust, 'strip-metadata result differs');
    assert(!wasm.text.includes('[seg 12'), 'strip-metadata left the prefix behind');
  });
}

function sleep(ms) {
  return new Promise((resolvePromise) => setTimeout(resolvePromise, ms));
}

async function mcpSession(serverPath, messages) {
  const child = spawn(process.execPath, [serverPath], {
    stdio: ['pipe', 'pipe', 'pipe'],
    windowsHide: true,
  });
  const replies = [];
  let stdout = '';
  let stderr = '';
  child.stdout.setEncoding('utf8');
  child.stderr.setEncoding('utf8');
  child.stdout.on('data', (chunk) => {
    stdout += chunk;
    let index;
    while ((index = stdout.indexOf('\n')) >= 0) {
      const line = stdout.slice(0, index).trim();
      stdout = stdout.slice(index + 1);
      if (!line) continue;
      try {
        replies.push(JSON.parse(line));
      } catch {
        stderr += `\n[non-JSON stdout] ${line}`;
      }
    }
  });
  child.stderr.on('data', (chunk) => {
    stderr += chunk;
  });

  const exit = new Promise((resolvePromise, rejectPromise) => {
    const timer = setTimeout(() => {
      child.kill();
      rejectPromise(new Error(`MCP server did not exit; stderr: ${stderr.slice(0, 500)}`));
    }, 15000);
    child.on('error', (err) => {
      clearTimeout(timer);
      rejectPromise(err);
    });
    child.on('exit', (code) => {
      clearTimeout(timer);
      resolvePromise(code);
    });
  });

  for (const message of messages) child.stdin.write(`${JSON.stringify(message)}\n`);
  const expected = messages
    .filter((message) => Object.prototype.hasOwnProperty.call(message, 'id'))
    .map((message) => message.id);
  const deadline = Date.now() + 10000;
  while (expected.some((id) => !replies.some((reply) => reply.id === id)) && Date.now() < deadline) {
    await sleep(20);
  }
  child.stdin.end();
  const exitCode = await exit;
  await sleep(20);
  return { replies, stderr, exitCode, expected };
}

function replyFor(replies, id) {
  const reply = replies.find((candidate) => candidate.id === id);
  assert(reply, `missing reply for id ${id}`);
  return reply;
}

function toolRequests(offset = 0) {
  const id = (n) => offset + n;
  return {
    ids: {
      initialize: id(1),
      tools: id(2),
      resources: id(3),
      check: id(4),
      fix: id(5),
      unknownMethod: id(6),
      invalidMode: id(7),
      nul: id(8),
      unknownTool: id(9),
      read: id(10),
      ping: id(11),
    },
    messages: [
      {
        jsonrpc: '2.0',
        id: id(1),
        method: 'initialize',
        params: {
          protocolVersion: '2025-06-18',
          capabilities: {},
          clientInfo: { name: 'verify.mjs', version: '1' },
        },
      },
      { jsonrpc: '2.0', method: 'notifications/initialized' },
      { jsonrpc: '2.0', id: id(2), method: 'tools/list' },
      { jsonrpc: '2.0', id: id(3), method: 'resources/list' },
      {
        jsonrpc: '2.0',
        id: id(4),
        method: 'tools/call',
        params: { name: 'check_prompt_format', arguments: { text: 'User: hi\n\n\nAssistant:' } },
      },
      {
        jsonrpc: '2.0',
        id: id(5),
        method: 'tools/call',
        params: { name: 'fix_prompt_format', arguments: { text: 'User: hi\n\n\nAssistant:', mode: 'both' } },
      },
      { jsonrpc: '2.0', id: id(6), method: 'no/such_method' },
      {
        jsonrpc: '2.0',
        id: id(7),
        method: 'tools/call',
        params: { name: 'fix_prompt_format', arguments: { text: 'User: hi', mode: 'nope' } },
      },
      {
        jsonrpc: '2.0',
        id: id(8),
        method: 'tools/call',
        params: { name: 'check_prompt_format', arguments: { text: 'User: a\u0000b' } },
      },
      {
        jsonrpc: '2.0',
        id: id(9),
        method: 'tools/call',
        params: { name: 'no_such_tool', arguments: {} },
      },
      {
        jsonrpc: '2.0',
        id: id(10),
        method: 'resources/read',
        params: { uri: 'rwkv-prompt-format://rules' },
      },
      { jsonrpc: '2.0', id: id(11), method: 'ping' },
    ],
  };
}

async function mcpSuite() {
  const { ids, messages } = toolRequests();
  const session = await mcpSession(MCP_SERVER, messages);
  strictEqual(session.exitCode, 0, `MCP server exited ${session.exitCode}: ${session.stderr.slice(0, 500)}`);
  strictEqual(session.replies.length, session.expected.length, 'unexpected extra or missing MCP replies');
  assert(!session.replies.some((reply) => reply.id === null), 'a notification received a reply');

  await gateAsync('mcp/initialize', () => {
    const init = replyFor(session.replies, ids.initialize).result;
    strictEqual(init.protocolVersion, '2025-06-18');
    strictEqual(init.serverInfo.name, 'rwkv-prompt-format');
    assert(init.instructions.includes('check_prompt_format'), 'server instructions do not mention check tool');
    assert(init.capabilities.tools && init.capabilities.resources, 'missing tools/resources capabilities');
  });

  await gateAsync('mcp/tools-list', () => {
    const tools = replyFor(session.replies, ids.tools).result.tools;
    deepStrictEqual(
      tools.map((tool) => tool.name).sort(),
      ['check_prompt_format', 'fix_prompt_format', 'get_prompt_format_rules'],
    );
    for (const tool of tools) {
      assert(tool.title && tool.description, `${tool.name} is missing title/description`);
      assert(tool.annotations, `${tool.name} is missing annotations`);
      assert(tool.outputSchema, `${tool.name} is missing outputSchema`);
      assert(!tool.inputSchema.additionalProperties, `${tool.name} inputSchema allows extra properties`);
    }
  });

  gate('mcp/check', () => {
    const result = replyFor(session.replies, ids.check).result;
    strictEqual(result.isError, false);
    strictEqual(result.structuredContent.ok, false);
    assert(result.structuredContent.findings.length > 0, 'check returned no findings for a broken prompt');
    assert(Array.isArray(result.content) && result.content[0].type === 'text', 'check content is not text');
  });

  gate('mcp/fix', () => {
    const result = replyFor(session.replies, ids.fix).result;
    strictEqual(result.isError, false);
    strictEqual(result.structuredContent.after.ok, true);
    assert(result.structuredContent.text.includes('Assistant:'), 'fixed text lost the generation point');
    assert(!result.structuredContent.text.includes('\n\n\n'), 'fixed text still has double blank lines');
  });

  gate('mcp/resources', () => {
    const list = replyFor(session.replies, ids.resources).result.resources;
    strictEqual(list.length, 1);
    strictEqual(list[0].uri, 'rwkv-prompt-format://rules');
    const read = replyFor(session.replies, ids.read).result.contents[0];
    assert(read.text.includes('## Hard rules'), 'rules resource is missing the hard-rules section');
  });

  gate('mcp/errors', () => {
    strictEqual(replyFor(session.replies, ids.unknownMethod).error.code, -32601);
    strictEqual(replyFor(session.replies, ids.unknownTool).error.code, -32602);
    strictEqual(replyFor(session.replies, ids.invalidMode).result.isError, true);
    strictEqual(replyFor(session.replies, ids.nul).result.isError, true);
    deepStrictEqual(replyFor(session.replies, ids.ping).result, {});
  });
}

function runInstaller(args) {
  return spawnSync(process.execPath, [INSTALLER, ...args], {
    encoding: 'utf8',
    maxBuffer: 16 * 1024 * 1024,
    windowsHide: true,
  });
}

function installerJson(result, label) {
  try {
    return JSON.parse(result.stdout);
  } catch {
    throw new Error(`${label} did not emit JSON (exit ${result.status}): ${result.stdout.slice(0, 300)}`);
  }
}

function readSettings(file) {
  return JSON.parse(readFileSync(file, 'utf8'));
}

async function installerSuite() {
  const root = mkdtempSync(join(os.tmpdir(), 'rwkv-pf-verify-'));
  try {
    const home = join(root, 'qoder-home');
    const skillDir = join(home, 'skills', 'rwkv-prompt-format');
    const settingsFile = join(home, 'settings.json');
    const installArgs = ['--scope', 'user', '--qoder-home', home, '--yes', '--json'];
    mkdirSync(home, { recursive: true });
    writeFileSync(
      settingsFile,
      `${JSON.stringify({ enabledPlugins: { 'qoder-context@qoderapp-bundler': true }, env: { VERIFY_KEEP: '1' } }, null, 2)}\n`,
      'utf8',
    );

    const first = runInstaller(installArgs);
    const installed = installerJson(first, 'install');
    gate('installer/install', () => {
      strictEqual(first.status, 0, first.stderr);
      strictEqual(installed.ok, true);
      strictEqual(installed.status.skill, 'copied');
      strictEqual(installed.status.mcp, 'add');
      for (const relative of [
        'SKILL.md',
        'references/rules.md',
        'mcp/server.mjs',
        'wasm/index.mjs',
        'wasm/cli.mjs',
        'wasm/rwkv_prompt_format.wasm',
      ]) {
        assert(existsSync(join(skillDir, relative)), `installed skill is missing ${relative}`);
      }
      const skillText = readFileSync(join(skillDir, 'SKILL.md'), 'utf8');
      assert(!skillText.includes('{{SKILL_DIR}}'), 'unresolved {{SKILL_DIR}} placeholder');
      assert(skillText.includes(skillDir.replaceAll('\\', '/')), 'installed SKILL.md has the wrong skill path');
    });

    gate('installer/settings-merge', () => {
      const settings = readSettings(settingsFile);
      const entry = settings.mcpServers['rwkv-prompt-format'];
      strictEqual(entry.command, process.execPath);
      deepStrictEqual(entry.args, [join(skillDir, 'mcp', 'server.mjs')]);
      deepStrictEqual(settings.enabledPlugins, { 'qoder-context@qoderapp-bundler': true });
      strictEqual(settings.env?.VERIFY_KEEP, '1', 'installer dropped unrelated settings');
    });

    const second = runInstaller(installArgs);
    gate('installer/idempotent', () => {
      const again = installerJson(second, 'reinstall');
      strictEqual(second.status, 0, second.stderr);
      strictEqual(again.status.mcp, 'unchanged');
      strictEqual(again.status.skill, 'copied');
      const entry = readSettings(settingsFile).mcpServers['rwkv-prompt-format'];
      deepStrictEqual(entry.args, [join(skillDir, 'mcp', 'server.mjs')]);
    });

    await gateAsync('installer/installed-server', async () => {
      const { ids, messages } = toolRequests(100);
      const session = await mcpSession(join(skillDir, 'mcp', 'server.mjs'), messages.slice(0, 6));
      strictEqual(session.exitCode, 0, session.stderr);
      const init = replyFor(session.replies, ids.initialize).result;
      strictEqual(init.serverInfo.name, 'rwkv-prompt-format');
      const fix = replyFor(session.replies, ids.fix).result;
      strictEqual(fix.structuredContent.after.ok, true);
    });

    gate('installer/bad-settings', () => {
      const badHome = join(root, 'bad-home');
      mkdirSync(badHome, { recursive: true });
      writeFileSync(join(badHome, 'settings.json'), '{ "mcpServers": ', { encoding: 'utf8' });
      const bad = runInstaller(['--scope', 'user', '--qoder-home', badHome, '--yes', '--json']);
      strictEqual(bad.status, 2, 'installer accepted broken settings JSON');
      assert(!existsSync(join(badHome, 'skills', 'rwkv-prompt-format')), 'half-installed skill after bad settings');
      strictEqual(readFileSync(join(badHome, 'settings.json'), 'utf8'), '{ "mcpServers": ');
    });

    gate('installer/not-owned', () => {
      const otherHome = join(root, 'other-home');
      mkdirSync(otherHome, { recursive: true });
      writeFileSync(
        join(otherHome, 'settings.json'),
        JSON.stringify({
          mcpServers: { 'rwkv-prompt-format': { command: 'node', args: ['/somewhere/else/server.mjs'] } },
        }),
        'utf8',
      );
      const foreign = runInstaller(['--scope', 'user', '--qoder-home', otherHome, '--uninstall', '--yes', '--json']);
      const report = installerJson(foreign, 'foreign uninstall');
      strictEqual(foreign.status, 1, 'uninstall of a foreign entry should fail');
      strictEqual(report.ok, false);
      strictEqual(report.status.mcp, 'not-owned');
      assert(
        readSettings(join(otherHome, 'settings.json')).mcpServers['rwkv-prompt-format'],
        'uninstall removed an entry it does not own',
      );
    });

    const removed = runInstaller(['--scope', 'user', '--qoder-home', home, '--uninstall', '--yes', '--json']);
    gate('installer/uninstall', () => {
      const report = installerJson(removed, 'uninstall');
      strictEqual(removed.status, 0, removed.stderr);
      strictEqual(report.status.mcp, 'remove');
      strictEqual(report.status.skill, 'remove');
      assert(!existsSync(skillDir), 'skill directory survived uninstall');
      const settings = readSettings(settingsFile);
      assert(!settings.mcpServers || !settings.mcpServers['rwkv-prompt-format'], 'MCP entry survived uninstall');
    });
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

function prerequisites() {
  const missing = [];
  if (!existsSync(RUST_CLI)) missing.push(`${RUST_CLI} (run cargo build --release)`);
  if (!existsSync(WASM_FILE)) missing.push(`${WASM_FILE} (run node scripts/build-wasm.mjs)`);
  if (!existsSync(MCP_SERVER)) missing.push(MCP_SERVER);
  if (!existsSync(INSTALLER)) missing.push(INSTALLER);
  return missing;
}

async function main() {
  const missing = prerequisites();
  if (missing.length > 0) {
    console.error('verify: missing prerequisites:');
    for (const item of missing) console.error(`  - ${item}`);
    process.exitCode = 2;
    return;
  }

  paritySuite();
  await mcpSuite();
  await installerSuite();

  console.log(`verify: ${passed} checks passed, ${failures.length} failed (${corpus.length} corpus inputs${QUICK ? ', quick' : ''})`);
  if (failures.length > 0) {
    for (const failure of failures) {
      console.error(`\nFAIL ${failure.name}`);
      console.error(`  ${failure.error instanceof Error ? failure.error.message : failure.error}`);
    }
    process.exitCode = 1;
  }
}

await main();
