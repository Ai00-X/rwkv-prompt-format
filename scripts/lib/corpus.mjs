// 检测/修复语料（check + fix 复用同一批输入）。
//
// 这里是「行为契约」的样本集：verify.mjs 用它做 Rust ↔ WASM 对拍，
// check-wasm-fresh.mjs 用它做「已提交产物 ↔ 现编产物」对拍。
// 改语料就是改契约，两处门禁会一起变。

export const CORPUS = [
  ['chat-ok', 'User: hi\n\nAssistant:'],
  ['messy-labels', 'user : hi\n\n\nassistant : '],
  [
    'chatml',
    'system\nYou are helpful.\n' +
      'user\nHello\nassistant\n',
  ],
  [
    'llama3',
    'system\n\nBe nice' +
      'user\n\nHi' +
      'assistant\n\n',
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

export const QUICK_NAMES = new Set(['chat-ok', 'messy-labels', 'chatml', 'material-qa', 'bom-crlf']);

/** 完整语料或快速子集。 */
export function pickCorpus(quick) {
  return quick ? CORPUS.filter(([name]) => QUICK_NAMES.has(name)) : CORPUS;
}
