#!/usr/bin/env node
/**
 * One-command installer for the rwkv-prompt-format skill and MCP server.
 *
 * Installs a self-contained copy (SKILL.md, references, MCP server, WASM
 * wrapper + artifact) and merges an entry into the Qoder settings file.
 * No network, no dependencies, no changes outside the selected scope.
 *
 * Examples:
 *   node scripts/install-agent.mjs --scope user --yes
 *   node scripts/install-agent.mjs --scope project --yes
 *   node scripts/install-agent.mjs --scope local --yes
 *   node scripts/install-agent.mjs --scope user --dry-run
 *   node scripts/install-agent.mjs --scope user --uninstall --yes
 *   node scripts/install-agent.mjs --scope user --qoder-home /tmp/qh --yes
 */

import { cpSync, existsSync, mkdirSync, readFileSync, readdirSync, renameSync, rmSync, statSync, unlinkSync, writeFileSync, copyFileSync } from 'node:fs';
import os from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { createInterface } from 'node:readline/promises';
import { fileURLToPath } from 'node:url';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const SKILL_NAME = 'rwkv-prompt-format';
const MCP_NAME = 'rwkv-prompt-format';
const SOURCE_SKILL = join(ROOT, 'skills', SKILL_NAME);
const SOURCE_SERVER = join(ROOT, 'mcp', 'server.mjs');
const SOURCE_WASM = join(ROOT, 'wasm');
const RUNTIME_FILES = ['index.mjs', 'index.d.ts', 'cli.mjs', 'rwkv_prompt_format.wasm'];

function usage() {
  console.log(`Install the RWKV prompt-format skill and MCP server.

Usage:
  node scripts/install-agent.mjs [options]

Options:
  --scope user|project|local  Where to install (default: user).
  --project PATH              Project root for project/local (default: cwd).
  --qoder-home PATH           Qoder user config dir (default: $QODER_HOME or ~/.qoder-cn).
  --yes, -y                   Do not ask for confirmation.
  --dry-run                   Print the plan without writing anything.
  --uninstall                 Remove the MCP entry and the installed skill.
  --no-mcp                    Skip the MCP settings entry.
  --no-skill                  Skip copying the skill directory.
  --json                      Print a machine-readable result.
  --help, -h                  Show this help.

Exit codes: 0 success, 1 error, 2 invalid arguments or unusable settings JSON.`);
}

function parseArgs(argv) {
  const opts = {
    scope: 'user',
    project: process.cwd(),
    qoderHome: process.env.QODER_HOME || join(os.homedir(), '.qoder-cn'),
    yes: false,
    dryRun: false,
    uninstall: false,
    mcp: true,
    skill: true,
    json: false,
    help: false,
  };
  for (let i = 0; i < argv.length; i += 1) {
    const raw = argv[i];
    const [key, inline] = raw.startsWith('--') ? raw.slice(2).split('=', 2) : [raw, undefined];
    const value = () => {
      if (inline !== undefined) return inline;
      const next = argv[i + 1];
      if (next === undefined || next.startsWith('-')) fail(`--${key} requires a value`);
      i += 1;
      return next;
    };
    switch (key) {
      case 'scope':
        opts.scope = value();
        break;
      case 'project':
        opts.project = resolve(value());
        break;
      case 'qoder-home':
        opts.qoderHome = resolve(value());
        break;
      case 'yes':
      case 'y':
      case '-y':
        opts.yes = true;
        break;
      case 'dry-run':
        opts.dryRun = true;
        break;
      case 'uninstall':
        opts.uninstall = true;
        break;
      case 'no-mcp':
        opts.mcp = false;
        break;
      case 'no-skill':
        opts.skill = false;
        break;
      case 'json':
        opts.json = true;
        break;
      case 'help':
      case 'h':
      case '-h':
        opts.help = true;
        break;
      default:
        fail(`unknown option: ${raw}`);
    }
  }
  if (!['user', 'project', 'local'].includes(opts.scope)) {
    fail('--scope must be user, project or local');
  }
  return opts;
}

function fail(message) {
  console.error(`rwkv-prompt-format installer: ${message}`);
  process.exit(2);
}

function pathsFor(opts) {
  const settingsDir = opts.scope === 'user' ? opts.qoderHome : join(opts.project, '.qoder');
  const settings = join(
    settingsDir,
    opts.scope === 'local' ? 'settings.local.json' : 'settings.json',
  );
  const skillDir = opts.scope === 'user'
    ? join(opts.qoderHome, 'skills', SKILL_NAME)
    : join(opts.project, '.qoder', 'skills', SKILL_NAME);
  return { settings, skillDir, server: join(skillDir, 'mcp', 'server.mjs') };
}

function readSettings(file) {
  if (!existsSync(file)) return {};
  let text = readFileSync(file, 'utf8');
  if (text.charCodeAt(0) === 0xfeff) text = text.slice(1);
  if (!text.trim()) return {};
  let value;
  try {
    value = JSON.parse(text);
  } catch (err) {
    fail(`cannot parse ${file}: ${err.message}. Nothing was changed.`);
  }
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    fail(`${file} is not a JSON object. Nothing was changed.`);
  }
  if (value.mcpServers !== undefined) {
    if (!value.mcpServers || typeof value.mcpServers !== 'object' || Array.isArray(value.mcpServers)) {
      fail(`${file}: "mcpServers" is not an object. Nothing was changed.`);
    }
  }
  return value;
}

function atomicWrite(file, content) {
  mkdirSync(dirname(file), { recursive: true });
  const temp = `${file}.tmp-${process.pid}`;
  writeFileSync(temp, content, 'utf8');
  try {
    renameSync(temp, file);
  } catch (err) {
    // Windows can refuse rename over an open file; fall back to an in-place copy.
    try {
      copyFileSync(temp, file);
      unlinkSync(temp);
    } catch {
      try { unlinkSync(temp); } catch {}
      throw err;
    }
  }
}

function backup(file) {
  if (!existsSync(file)) return null;
  const target = `${file}.rwkv-pf.bak`;
  copyFileSync(file, target);
  return target;
}

function copySkill(opts, dest) {
  if (!existsSync(SOURCE_SKILL)) fail(`missing source skill: ${SOURCE_SKILL}`);
  if (opts.dryRun) return;
  cpSync(SOURCE_SKILL, dest, { recursive: true, force: true });
  const dir = dest.replaceAll('\\', '/');
  for (const file of markdownFiles(dest)) {
    const text = readFileSync(file, 'utf8');
    if (text.includes('{{SKILL_DIR}}')) {
      writeFileSync(file, text.replaceAll('{{SKILL_DIR}}', dir), 'utf8');
    }
  }
}

function markdownFiles(dir) {
  const out = [];
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) out.push(...markdownFiles(full));
    else if (entry.endsWith('.md')) out.push(full);
  }
  return out;
}

function copyRuntime(opts, dest) {
  if (opts.dryRun) return;
  mkdirSync(join(dest, 'mcp'), { recursive: true });
  mkdirSync(join(dest, 'wasm'), { recursive: true });
  cpSync(SOURCE_SERVER, join(dest, 'mcp', 'server.mjs'), { force: true });
  for (const name of RUNTIME_FILES) {
    cpSync(join(SOURCE_WASM, name), join(dest, 'wasm', name), { force: true });
  }
}

function plan(opts, p) {
  const actions = [];
  if (opts.skill) {
    actions.push(`${opts.uninstall ? 'remove' : 'copy'} skill -> ${p.skillDir}`);
    if (!opts.uninstall) actions.push(`copy MCP server + WASM -> ${p.skillDir}`);
  }
  if (opts.mcp) {
    actions.push(`${opts.uninstall ? 'remove' : 'merge'} mcpServers["${MCP_NAME}"] -> ${p.settings}`);
  }
  return actions;
}

async function confirm(opts) {
  if (opts.yes || opts.dryRun) return;
  if (!process.stdin.isTTY) {
    fail('non-interactive input; pass --yes to confirm');
  }
  const rl = createInterface({ input: process.stdin, output: process.stderr });
  const answer = await rl.question('Proceed? [y/N] ');
  rl.close();
  if (!/^y(es)?$/i.test(answer.trim())) {
    console.error('Cancelled.');
    process.exit(0);
  }
}

function entryFor(p) {
  return { command: process.execPath, args: [p.server] };
}

function sameEntry(a, b) {
  return JSON.stringify(a) === JSON.stringify(b);
}

function installMcp(opts, p) {
  const settings = readSettings(p.settings);
  const servers = settings.mcpServers ?? {};
  const entry = entryFor(p);
  const current = servers[MCP_NAME];
  if (sameEntry(current, entry)) {
    return { changed: false, settings, backup: null, status: 'unchanged' };
  }
  if (opts.dryRun) {
    return { changed: true, settings, backup: null, status: current ? 'update' : 'add' };
  }
  const bak = backup(p.settings);
  servers[MCP_NAME] = entry;
  settings.mcpServers = servers;
  atomicWrite(p.settings, `${JSON.stringify(settings, null, 2)}\n`);
  return { changed: true, settings, backup: bak, status: current ? 'update' : 'add' };
}

function removeMcp(opts, p) {
  const settings = readSettings(p.settings);
  const servers = settings.mcpServers;
  if (!servers || !(MCP_NAME in servers)) {
    return { changed: false, backup: null, status: 'absent' };
  }
  const entry = servers[MCP_NAME];
  const target = p.skillDir.replaceAll('\\', '/').toLowerCase();
  const owned = entry && Array.isArray(entry.args) && entry.args.some((a) =>
    String(a).replaceAll('\\', '/').toLowerCase().includes(target),
  );
  if (!owned) {
    return { changed: false, backup: null, status: 'not-owned' };
  }
  if (opts.dryRun) return { changed: true, backup: null, status: 'remove' };
  const bak = backup(p.settings);
  delete servers[MCP_NAME];
  if (Object.keys(servers).length === 0) delete settings.mcpServers;
  atomicWrite(p.settings, `${JSON.stringify(settings, null, 2)}\n`);
  return { changed: true, backup: bak, status: 'remove' };
}

function removeSkill(opts, p) {
  if (!existsSync(p.skillDir)) return 'absent';
  const marker = join(p.skillDir, 'SKILL.md');
  if (!existsSync(marker) || !readFileSync(marker, 'utf8').includes(`name: ${SKILL_NAME}`)) {
    return 'not-owned';
  }
  if (opts.dryRun) return 'remove';
  rmSync(p.skillDir, { recursive: true, force: true });
  return 'remove';
}

function nextSteps() {
  return [
    'run /mcp reload',
    'run /skills reload (or restart the Qoder session)',
    `try: 检查并修复这个 RWKV 提示词：<paste a prompt>`,
  ];
}

async function main() {
  const opts = parseArgs(process.argv.slice(2));
  if (opts.help) {
    usage();
    return;
  }
  if (!existsSync(SOURCE_SERVER)) fail(`missing server: ${SOURCE_SERVER}`);
  if (!existsSync(join(SOURCE_WASM, 'rwkv_prompt_format.wasm'))) {
    fail('missing WASM artifact; run node scripts/build-wasm.mjs first');
  }
  for (const name of RUNTIME_FILES) {
    if (!existsSync(join(SOURCE_WASM, name))) fail(`missing WASM file: ${join(SOURCE_WASM, name)}`);
  }
  if (opts.skill && !existsSync(SOURCE_SKILL)) fail(`missing source skill: ${SOURCE_SKILL}`);

  const p = pathsFor(opts);
  // Validate settings before copying anything, so a broken config cannot leave
  // a half-installed skill behind.
  if (opts.mcp) readSettings(p.settings);
  const actions = plan(opts, p);
  if (!opts.json) {
    console.log(`rwkv-prompt-format installer (${opts.scope}${opts.dryRun ? ', dry-run' : ''})`);
    for (const action of actions) console.log(`  - ${action}`);
  }
  await confirm(opts);

  const result = {
    ok: true,
    action: opts.uninstall ? 'uninstall' : 'install',
    scope: opts.scope,
    settings: p.settings,
    skill: p.skillDir,
    server: p.server,
    changed: false,
    backup: null,
    status: {},
    next: [],
  };

  if (opts.uninstall) {
    if (opts.mcp) result.status.mcp = removeMcp(opts, p).status;
    if (opts.skill) result.status.skill = removeSkill(opts, p);
    result.changed = result.status.mcp === 'remove' || result.status.skill === 'remove';
  } else {
    if (opts.skill) {
      copySkill(opts, p.skillDir);
      copyRuntime(opts, p.skillDir);
      result.status.skill = opts.dryRun ? 'would-copy' : 'copied';
    }
    if (opts.mcp) {
      const merged = installMcp(opts, p);
      result.status.mcp = merged.status;
      result.changed = merged.changed || opts.skill;
      result.backup = merged.backup;
    } else if (opts.skill) {
      result.changed = true;
    }
    result.next = nextSteps();
  }

  if (result.status.mcp === 'not-owned' || result.status.skill === 'not-owned') {
    result.ok = false;
  }

  if (opts.json) {
    console.log(JSON.stringify(result, null, 2));
  } else {
    console.log('');
    if (opts.dryRun) {
      console.log('Dry run complete (nothing written).');
    } else {
      console.log(opts.uninstall ? 'Uninstall complete.' : 'Installed.');
    }
    console.log(`  skill:  ${p.skillDir}`);
    if (opts.mcp) console.log(`  config: ${p.settings} (${result.status.mcp})`);
    if (result.backup) console.log(`  backup: ${result.backup}`);
    if (!opts.uninstall) {
      console.log('');
      console.log('Next:');
      for (const step of result.next) console.log(`  - ${step}`);
      console.log('');
      console.log('One-prompt install (paste into an agent):');
      console.log(`  node "${join(ROOT, 'scripts', 'install-agent.mjs')}" --scope ${opts.scope} --yes`);
    }
  }
  if (!result.ok) {
    console.error('rwkv-prompt-format installer: left an entry that this installer does not own');
    process.exitCode = 1;
  }
}

try {
  await main();
} catch (err) {
  console.error(`rwkv-prompt-format installer: ${err instanceof Error ? err.message : err}`);
  process.exitCode = 1;
}
