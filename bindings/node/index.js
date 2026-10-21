'use strict';

/**
 * RWKV G1/G1x prompt format checker and auto-fixer (Node.js binding).
 *
 * Requires the `koffi` FFI package:  pnpm add koffi
 * The native library is looked up via RWKV_PROMPT_FORMAT_LIB, then
 * <repo>/target/release/, then the system loader path.
 *
 *   const pf = require('rwkv-prompt-format');
 *   const report = pf.check(text);      // parsed FormatReport
 *   const result = pf.fix(text);        // parsed FixResult (result.text)
 *   const fixed  = pf.fixText(text);    // string, no JSON round trip
 */

const path = require('node:path');

const FIX_SAFE = 0x1;
const FIX_STRUCTURAL = 0x2;
const FIX_STRIP_METADATA = 0x4;
const FIX_ALL = FIX_SAFE | FIX_STRUCTURAL;

const LIB_NAMES = {
  win32: ['rwkv_prompt_format.dll'],
  darwin: ['librwkv_prompt_format.dylib'],
  linux: ['librwkv_prompt_format.so'],
};

function candidates() {
  const names = LIB_NAMES[process.platform] || LIB_NAMES.linux;
  const out = [];
  if (process.env.RWKV_PROMPT_FORMAT_LIB) out.push(process.env.RWKV_PROMPT_FORMAT_LIB);
  const repoRoot = path.resolve(__dirname, '..', '..');
  for (const name of names) out.push(path.join(repoRoot, 'target', 'release', name));
  out.push(names[0]);
  return out;
}

let koffi;
try {
  koffi = require('koffi');
} catch (err) {
  throw new Error(
    'rwkv-prompt-format needs the "koffi" package (pnpm add koffi). ' +
      'Alternatively call the rwkv-prompt-format CLI from any language.',
  );
}

let lib;
let lastError;
for (const candidate of candidates()) {
  try {
    lib = koffi.load(candidate);
    break;
  } catch (err) {
    lastError = err;
  }
}
if (!lib) {
  throw new Error(
    'cannot load rwkv_prompt_format; set RWKV_PROMPT_FORMAT_LIB. Last error: ' +
      (lastError && lastError.message),
  );
}

// koffi converts `char *` returns to JS strings (and then we cannot free
// them), so the owning functions return `void *` and are decoded manually.
const versionFn = lib.func('const char *rwkv_pf_version(void)');
const checkFn = lib.func('void *rwkv_pf_check(const char *text_utf8)');
const fixFn = lib.func('void *rwkv_pf_fix(const char *text_utf8, uint32_t flags)');
const fixTextFn = lib.func('void *rwkv_pf_fix_text(const char *text_utf8, uint32_t flags)');
const freeFn = lib.func('void rwkv_pf_free_string(void *ptr)');

function take(ptr) {
  if (!ptr) return null;
  try {
    return koffi.decode(ptr, 'char', -1);
  } finally {
    freeFn(ptr);
  }
}

function parse(raw) {
  return JSON.parse(raw === null ? '{"error":"null result"}' : raw);
}

function version() {
  return versionFn();
}

function check(text) {
  return parse(take(checkFn(String(text))));
}

function fix(text, flags = FIX_ALL) {
  return parse(take(fixFn(String(text), flags >>> 0)));
}

function fixText(text, flags = FIX_ALL) {
  return take(fixTextFn(String(text), flags >>> 0));
}

module.exports = {
  FIX_SAFE,
  FIX_STRUCTURAL,
  FIX_STRIP_METADATA,
  FIX_ALL,
  version,
  check,
  fix,
  fixText,
};
