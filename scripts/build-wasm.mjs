#!/usr/bin/env node
// 构建 wasm32 产物并复制到 wasm/rwkv_prompt_format.wasm。
// 用法：node scripts/build-wasm.mjs [--check] [--out PATH]
//   --check    只校验已提交的 .wasm 是否与当前源码构建结果一致（字节级）。
//   --out PATH 产物写到 PATH 而不是 wasm/（CI 新鲜度门禁用）。

import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { copyFileSync, existsSync, mkdirSync, readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const target = join(root, 'target', 'wasm32-unknown-unknown', 'wasm-release', 'rwkv_prompt_format.wasm');
// --out <path>：把产物写到别处（CI 的新鲜度门禁用），默认写回 wasm/。
const outIndex = process.argv.indexOf('--out');
const output =
  outIndex >= 0 && process.argv[outIndex + 1]
    ? resolve(process.argv[outIndex + 1])
    : join(root, 'wasm', 'rwkv_prompt_format.wasm');
const checkOnly = process.argv.includes('--check');

const build = spawnSync(
  'cargo',
  ['build', '--profile', 'wasm-release', '--target', 'wasm32-unknown-unknown', '--lib', '--no-default-features'],
  { cwd: root, stdio: 'inherit', shell: process.platform === 'win32' },
);
if (build.status !== 0) {
  console.error('cargo build failed');
  process.exit(build.status ?? 1);
}
if (!existsSync(target)) {
  console.error(`missing build artifact: ${target}`);
  process.exit(1);
}

const built = readFileSync(target);
const hash = createHash('sha256').update(built).digest('hex');
if (checkOnly) {
  if (!existsSync(output)) {
    console.error(`missing committed wasm: ${output}`);
    process.exit(1);
  }
  const committed = readFileSync(output);
  const committedHash = createHash('sha256').update(committed).digest('hex');
  if (committedHash !== hash) {
    console.error(`wasm is stale: committed ${committedHash}, built ${hash}`);
    process.exit(1);
  }
  console.log(`OK wasm up to date: ${built.length} bytes, sha256 ${hash}`);
  process.exit(0);
}

mkdirSync(dirname(output), { recursive: true });
copyFileSync(target, output);
console.log(`wasm: ${output}`);
console.log(`size: ${built.length} bytes`);
console.log(`sha256: ${hash}`);
