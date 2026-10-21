/**
 * RWKV G1/G1x prompt format checker/fixer — zero-dependency WASM wrapper.
 *
 * Node:
 *   import { check, fix } from './wasm/index.mjs';
 *   const report = await check(text);
 *
 * Browser:
 *   import { init, check } from './index.mjs';
 *   await init(new URL('./rwkv_prompt_format.wasm', import.meta.url));
 *
 * The module has no imports and needs no wasm-bindgen; it exposes the same
 * check/fix/fix_text/free_string functions as the C ABI plus a tiny allocator.
 */

const ABI_VERSION = 1;

export const FIX_SAFE = 0x1;
export const FIX_STRUCTURAL = 0x2;
export const FIX_STRIP_METADATA = 0x4;
export const FIX_ALL = FIX_SAFE | FIX_STRUCTURAL;

const isNode = typeof process !== 'undefined' && Boolean(process.versions?.node);

let state = null;
let initPromise = null;

function error(message, cause) {
  return cause === undefined ? new Error(message) : new Error(message, { cause });
}

async function loadBytes(source) {
  if (source === undefined) {
    const url = new URL('./rwkv_prompt_format.wasm', import.meta.url);
    if (isNode) {
      const { readFile } = await import('node:fs/promises');
      return new Uint8Array(await readFile(url));
    }
    const response = await fetch(url);
    if (!response.ok) throw error(`failed to fetch ${url}: HTTP ${response.status}`);
    return new Uint8Array(await response.arrayBuffer());
  }
  if (typeof Response !== 'undefined' && source instanceof Response) {
    return new Uint8Array(await source.arrayBuffer());
  }
  if (source instanceof ArrayBuffer) {
    return new Uint8Array(source);
  }
  if (ArrayBuffer.isView(source)) {
    return new Uint8Array(source.buffer, source.byteOffset, source.byteLength);
  }
  if (source instanceof URL || typeof source === 'string') {
    const text = String(source);
    if (/^https?:\/\//i.test(text)) {
      const response = await fetch(text);
      if (!response.ok) throw error(`failed to fetch ${text}: HTTP ${response.status}`);
      return new Uint8Array(await response.arrayBuffer());
    }
    if (!isNode) {
      const response = await fetch(source);
      if (!response.ok) throw error(`failed to fetch ${text}: HTTP ${response.status}`);
      return new Uint8Array(await response.arrayBuffer());
    }
    const { readFile } = await import('node:fs/promises');
    return new Uint8Array(await readFile(source));
  }
  throw new TypeError('init(source): expected URL, path, Response, ArrayBuffer, view or WebAssembly.Module');
}

function bind(instance) {
  const ex = instance.exports;
  for (const name of [
    'memory',
    'rwkv_pf_check',
    'rwkv_pf_fix',
    'rwkv_pf_fix_text',
    'rwkv_pf_free_string',
    'rwkv_pf_version',
    'rwkv_pf_wasm_alloc',
    'rwkv_pf_wasm_free',
    'rwkv_pf_wasm_abi',
  ]) {
    if (!(name in ex)) throw error(`wasm module is missing export "${name}"`);
  }
  const abi = ex.rwkv_pf_wasm_abi();
  if (abi !== ABI_VERSION) {
    throw error(`wasm ABI ${abi} does not match wrapper ABI ${ABI_VERSION}`);
  }
  // ignoreBOM: true means "keep a leading U+FEFF" instead of silently dropping it.
  const decoder = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true });
  return {
    ex,
    memory: ex.memory,
    encoder: new TextEncoder(),
    decoder,
    version: readCStringStatic(ex, decoder, ex.rwkv_pf_version()),
  };
}

function readCStringStatic(ex, decoder, ptr) {
  if (!ptr) return '';
  const view = new Uint8Array(ex.memory.buffer);
  let end = ptr;
  while (end < view.length && view[end] !== 0) end += 1;
  return decoder.decode(view.subarray(ptr, end));
}

export function init(source) {
  if (state) return Promise.resolve();
  if (!initPromise) {
    initPromise = (async () => {
      let instance;
      if (source instanceof WebAssembly.Module) {
        instance = await WebAssembly.instantiate(source, {});
      } else {
        const bytes = await loadBytes(source);
        instance = (await WebAssembly.instantiate(bytes, {})).instance;
      }
      state = bind(instance);
      return state;
    })().catch((cause) => {
      initPromise = null;
      throw error(`failed to initialize rwkv-prompt-format wasm: ${cause.message}`, cause);
    });
  }
  return initPromise;
}

async function ready() {
  if (!state) await init();
  return state;
}

function readResult(ptr) {
  if (!ptr) throw error('wasm returned a null result');
  try {
    const view = new Uint8Array(state.memory.buffer);
    let end = ptr;
    while (end < view.length && view[end] !== 0) end += 1;
    return state.decoder.decode(view.subarray(ptr, end));
  } finally {
    state.ex.rwkv_pf_free_string(ptr);
  }
}

function withInput(text, fn) {
  if (typeof text !== 'string') throw new TypeError('prompt must be a string');
  if (text.includes('\0')) {
    throw new TypeError('prompt must not contain NUL characters (the C/WASM ABI is NUL-terminated)');
  }
  const bytes = state.encoder.encode(text);
  const len = bytes.length + 1; // alloc() is zero-filled: the last byte is the C NUL.
  const ptr = state.ex.rwkv_pf_wasm_alloc(len);
  if (!ptr) throw error('wasm allocation failed');
  try {
    new Uint8Array(state.memory.buffer, ptr, len).set(bytes);
    return fn(ptr);
  } finally {
    state.ex.rwkv_pf_wasm_free(ptr, len);
  }
}

function parse(raw) {
  return JSON.parse(raw);
}

export async function check(text) {
  await ready();
  return parse(withInput(text, (ptr) => readResult(state.ex.rwkv_pf_check(ptr))));
}

export async function fix(text, flags = FIX_ALL) {
  await ready();
  const n = flags >>> 0;
  return parse(withInput(text, (ptr) => readResult(state.ex.rwkv_pf_fix(ptr, n))));
}

export async function fixText(text, flags = FIX_ALL) {
  await ready();
  const n = flags >>> 0;
  return withInput(text, (ptr) => readResult(state.ex.rwkv_pf_fix_text(ptr, n)));
}

export async function version() {
  await ready();
  return state.version;
}

export async function abiVersion() {
  await ready();
  return state.ex.rwkv_pf_wasm_abi();
}
