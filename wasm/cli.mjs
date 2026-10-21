#!/usr/bin/env node
// 无需 Rust 的 Node CLI：直接跑 wasm，接口与 target/release/rwkv-pf 对齐。
// 用法：node wasm/cli.mjs check --stdin [--json]
//       node wasm/cli.mjs fix --file prompt.txt --in-place

import { readFileSync, writeFileSync } from 'node:fs';
import {
  FIX_ALL,
  FIX_SAFE,
  FIX_STRIP_METADATA,
  FIX_STRUCTURAL,
  check,
  fix,
  version,
} from './index.mjs';

const USAGE = `rwkv-pf-wasm — RWKV G1/G1x prompt format checker/fixer (WASM)

用法:
  node wasm/cli.mjs check (--file PATH | --text TEXT | --stdin) [--json] [--no-fail]
  node wasm/cli.mjs fix   (--file PATH | --text TEXT | --stdin) [选项]

fix 选项:
  --safe-only         只做安全修复
  --structural-only   只做结构修复
  --strip-metadata    去掉 [seg 12 · user] 元数据前缀
  --in-place          写回 --file
  --json              输出 FixResult JSON
  --quiet             不打印摘要
  --no-fail           修复后仍有 error 也返回 0

退出码: 0 成功 / 1 仍有 error / 2 参数或读取错误`;

function fail(message) {
  console.error(`错误：${message}`);
  process.exit(2);
}

function decode(buf, label) {
  try {
    return new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(buf);
  } catch {
    throw new Error(`${label} 不是有效的 UTF-8`);
  }
}

function parse(argv) {
  const command = argv.shift();
  if (!command || command === '--help' || command === '-h') {
    console.log(USAGE);
    process.exit(command ? 0 : 2);
  }
  if (command !== 'check' && command !== 'fix') fail(`未知命令 ${command}`);
  const opts = {
    command,
    file: null,
    text: null,
    stdin: false,
    json: false,
    noFail: false,
    quiet: false,
    safeOnly: false,
    structuralOnly: false,
    stripMetadata: false,
    inPlace: false,
  };
  const takeValue = (arg, i) => {
    if (i + 1 >= argv.length) fail(`${arg} 缺少值`);
    return argv[i + 1];
  };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    switch (arg) {
      case '--file':
        opts.file = takeValue(arg, i);
        i += 1;
        break;
      case '--text':
        opts.text = takeValue(arg, i);
        i += 1;
        break;
      case '--stdin':
        opts.stdin = true;
        break;
      case '--json':
        opts.json = true;
        break;
      case '--no-fail':
        opts.noFail = true;
        break;
      case '--quiet':
        opts.quiet = true;
        break;
      case '--safe-only':
        opts.safeOnly = true;
        break;
      case '--structural-only':
        opts.structuralOnly = true;
        break;
      case '--strip-metadata':
        opts.stripMetadata = true;
        break;
      case '--in-place':
        opts.inPlace = true;
        break;
      case '--help':
      case '-h':
        console.log(USAGE);
        process.exit(0);
        break;
      default:
        fail(`未知参数 ${arg}`);
    }
  }
  const sources = Number(opts.file !== null) + Number(opts.text !== null) + Number(opts.stdin);
  if (sources !== 1) fail('必须且只能指定 --file / --text / --stdin 之一');
  if (opts.safeOnly && opts.structuralOnly) fail('--safe-only 与 --structural-only 不能同时使用');
  if (opts.inPlace && (opts.command !== 'fix' || opts.file === null)) {
    fail('--in-place 只能用于 fix 且需要 --file');
  }
  return opts;
}

function readSource(opts) {
  if (opts.file !== null) return decode(readFileSync(opts.file), opts.file);
  if (opts.text !== null) return opts.text;
  return decode(readFileSync(0), 'stdin');
}

const label = (severity) => ({ error: 'E', warn: 'W', info: 'I' })[severity] ?? '?';

function printReport(report) {
  const status = report.ok ? 'OK' : 'FAIL';
  const gp = report.generation_point
    ? `L${report.generation_point.line} ${report.generation_point.role}${report.generation_point.seeded ? '(seed)' : ''}`
    : '无';
  console.log(
    `${status}（${report.error_count} error / ${report.warning_count} warn）· 格式 ${report.format} · 生成点 ${gp}`,
  );
  const s = report.stats;
  console.log(
    `统计：${s.chars} 字符 / ${s.lines} 行 / User ${s.user_blocks} · Assistant ${s.assistant_blocks} · ` +
      `System ${s.system_blocks} · Instruction ${s.instruction_blocks} · 空行分隔 ${s.blank_separators} · ` +
      `最大连续空行 ${s.max_blank_run} · 元数据行 ${s.metadata_lines}` +
      `${s.think_prefix ? ' · think' : s.function_call ? ' · tools' : ''}`,
  );
  if (report.findings.length === 0) {
    console.log('✓ 未发现格式问题');
    return;
  }
  for (const f of report.findings) {
    const loc = f.line === 0 ? '  -  ' : `L${String(f.line).padEnd(4)}`;
    console.log(`  ${loc} [${label(f.severity)}] ${f.code.padEnd(28)} ${f.message}`);
    if (f.evidence) console.log(`           │ ${f.evidence}`);
  }
}

function printFixSummary(result, inPlace) {
  const status = result.changed ? '已修改' : '未修改';
  console.error(
    `修复：${status} · 修复项 ${result.applied.length} 类 · 残余 ${result.after.error_count} error / ` +
      `${result.after.warning_count} warn${inPlace ? ' · 已写回文件' : ''}`,
  );
  for (const item of result.applied) {
    const severity = result.before.findings.find((f) => f.code === item.code)?.severity;
    console.error(`  [${severity ? label(severity) : 'I'}] ${item.code.padEnd(28)} ×${item.count} ${item.message}`);
  }
  for (const note of result.notes) console.error(`  说明：${note}`);
}

async function main() {
  const opts = parse(process.argv.slice(2));
  const text = readSource(opts);
  if (opts.command === 'check') {
    const report = await check(text);
    if (opts.json) console.log(JSON.stringify(report, null, 2));
    else printReport(report);
    process.exitCode = report.ok || opts.noFail ? 0 : 1;
    return;
  }

  let flags = FIX_ALL;
  if (opts.safeOnly) flags = FIX_SAFE;
  if (opts.structuralOnly) flags = FIX_STRUCTURAL;
  if (opts.stripMetadata) flags |= FIX_STRIP_METADATA;
  const result = await fix(text, flags);
  if (opts.inPlace) writeFileSync(opts.file, result.text, 'utf8');
  if (opts.json) console.log(JSON.stringify(result, null, 2));
  else if (!opts.inPlace) process.stdout.write(result.text);
  if (!opts.quiet) printFixSummary(result, opts.inPlace);
  process.exitCode = result.after.ok || opts.noFail ? 0 : 1;
}

if (process.argv.includes('--version') || process.argv.includes('-V')) {
  console.log(`rwkv-pf-wasm ${await version()}`);
  process.exitCode = 0;
} else {
  try {
    await main();
  } catch (err) {
    fail(err instanceof Error ? err.message : String(err));
  }
}
