#!/usr/bin/env node
/**
 * WASM 新鲜度门禁：已提交的 `wasm/rwkv_prompt_format.wasm` 是否还跟得上源码。
 *
 * 为什么不是纯字节比较：rustc 版本一换，wasm 产物的字节就可能变（路径、LLVM
 * 编码、metadata 布局），但**行为**完全一样。纯字节门禁会在每次工具链升级后
 * 无差别变红，而它本来想抓的是「源码改了、产物忘了重编」这件事。
 *
 * 所以分两层：
 *   1. 字节层（提示）：sha256 相同 = 完全一致；不同 = 工具链漂移，只提示不拦。
 *   2. 语义层（拦）：用同一批语料跑「已提交 wasm」与「现编 wasm」，逐字段对比
 *      check / fix(safe|structural|both) 的 JSON 输出。不一致 = 产物真过期，退出码 1。
 *
 * 用法：
 *   node scripts/check-wasm-fresh.mjs            # 完整语料
 *   node scripts/check-wasm-fresh.mjs --quick    # 快速子集
 *
 * 退出码：0 新鲜（或仅字节漂移）/ 1 已提交产物过期 / 2 构建失败
 */

import { spawnSync } from 'node:child_process';
import { deepStrictEqual } from 'node:assert';
import { createHash } from 'node:crypto';
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import os from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { pickCorpus } from './lib/corpus.mjs';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const QUICK = process.argv.includes('--quick');
const COMMITTED_WASM = join(ROOT, 'wasm', 'rwkv_prompt_format.wasm');
const COMMITTED_CLI = join(ROOT, 'wasm', 'cli.mjs');
const RUST_CLI = join(ROOT, 'target', 'release', process.platform === 'win32' ? 'rwkv-pf.exe' : 'rwkv-pf');

const CHECK_ARGS = ['check', '--stdin', '--json', '--no-fail'];
const FIX_ARGS = {
  safe: ['fix', '--stdin', '--json', '--quiet', '--no-fail', '--safe-only'],
  structural: ['fix', '--stdin', '--json', '--quiet', '--no-fail', '--structural-only'],
  both: ['fix', '--stdin', '--json', '--quiet', '--no-fail'],
};

function sha256(path) {
  return createHash('sha256').update(readFileSync(path)).digest('hex');
}

function run(command, args, input) {
  const result = spawnSync(command, args, {
    input,
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
    windowsHide: true,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`${command} ${args.join(' ')} exited ${result.status}: ${result.stderr.slice(0, 500)}`);
  }
  try {
    return JSON.parse(result.stdout);
  } catch {
    throw new Error(`${command} ${args.join(' ')} did not emit JSON: ${result.stdout.slice(0, 300)}`);
  }
}

function cargo(args) {
  const result = spawnSync('cargo', args, {
    cwd: ROOT,
    stdio: 'inherit',
    shell: process.platform === 'win32',
  });
  if (result.status !== 0) process.exit(2);
}

if (!existsSync(COMMITTED_WASM)) {
  console.error(`missing committed wasm: ${COMMITTED_WASM}`);
  process.exit(2);
}

// 1) 现编一份到临时目录（不碰仓库里的产物）。
const tmp = mkdtempSync(join(os.tmpdir(), 'rwkv-pf-wasm-fresh-'));
const freshDir = join(tmp, 'fresh');
mkdirSync(freshDir, { recursive: true });
const freshWasm = join(freshDir, 'rwkv_prompt_format.wasm');

try {
  const build = spawnSync(
    process.execPath,
    [join(ROOT, 'scripts', 'build-wasm.mjs'), '--out', freshWasm],
    { cwd: ROOT, stdio: 'inherit' },
  );
  if (build.status !== 0) {
    console.error('fresh wasm build failed');
    process.exit(2);
  }
  // cli.mjs / index.mjs 会按自身位置找同名 .wasm，所以整组复制过去。
  copyFileSync(COMMITTED_CLI, join(freshDir, 'cli.mjs'));
  copyFileSync(join(ROOT, 'wasm', 'index.mjs'), join(freshDir, 'index.mjs'));

  // 2) 字节层：只提示。
  const committedHash = sha256(COMMITTED_WASM);
  const freshHash = sha256(freshWasm);
  const bytesMatch = committedHash === freshHash;
  console.log(
    bytesMatch
      ? `bytes: identical (sha256 ${committedHash})`
      : `bytes: differ (committed ${committedHash}, fresh ${freshHash}) — toolchain drift, semantic gate decides`,
  );

  // 3) 语义层：三方对拍（Rust CLI / 已提交 wasm / 现编 wasm）。
  const cli = {
    rust: { command: RUST_CLI, args: (a) => a },
    committed: { command: process.execPath, args: (a) => [COMMITTED_CLI, ...a] },
    fresh: { command: process.execPath, args: (a) => [join(freshDir, 'cli.mjs'), ...a] },
  };
  const hasRust = existsSync(RUST_CLI);
  if (!hasRust) {
    console.log('rust: target/release/rwkv-pf not built, comparing wasm ↔ wasm only');
    cargo(['build', '--release']);
  }

  const corpus = pickCorpus(QUICK);
  let compared = 0;
  for (const [name, text] of corpus) {
    const variants = hasRust ? ['rust', 'committed', 'fresh'] : ['committed', 'fresh'];
    for (const [label, args] of [['check', CHECK_ARGS], ...Object.entries(FIX_ARGS)]) {
      const reports = variants.map((v) => run(cli[v].command, cli[v].args(args), text));
      for (let i = 1; i < reports.length; i += 1) {
        try {
          deepStrictEqual(reports[i], reports[0], `${label}/${name}: ${variants[i]} differs from ${variants[0]}`);
        } catch (err) {
          console.error(`STALE wasm: ${err.message}`);
          process.exit(1);
        }
      }
      compared += 1;
    }
  }
  console.log(`OK wasm is fresh: ${compared} comparisons over ${corpus.length} corpus inputs`);
} finally {
  rmSync(tmp, { recursive: true, force: true });
}
