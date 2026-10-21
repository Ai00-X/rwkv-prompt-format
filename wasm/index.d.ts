/** RWKV G1/G1x prompt format checker/fixer (WASM). */

export const FIX_SAFE: 0x1;
export const FIX_STRUCTURAL: 0x2;
export const FIX_STRIP_METADATA: 0x4;
export const FIX_ALL: 0x3;

export type Severity = 'error' | 'warn' | 'info';

export interface Finding {
  code: string;
  severity: Severity;
  /** 1-based; 0 means a whole-text finding. */
  line: number;
  message: string;
  evidence: string;
}

export interface GenerationPoint {
  line: number;
  role: string;
  seeded: boolean;
}

export interface FormatStats {
  chars: number;
  lines: number;
  user_blocks: number;
  assistant_blocks: number;
  system_blocks: number;
  instruction_blocks: number;
  input_blocks: number;
  response_blocks: number;
  blank_separators: number;
  max_blank_run: number;
  metadata_lines: number;
  think_prefix: boolean;
  function_call: boolean;
}

export interface FormatReport {
  profile: string;
  /** plain_chat | material_qa | instruction | instruction_chat | system_tools |
   *  chatml | markdown | jsonl | continuation | unknown */
  format: string;
  ok: boolean;
  error_count: number;
  warning_count: number;
  info_count: number;
  generation_point: GenerationPoint | null;
  stats: FormatStats;
  findings: Finding[];
}

export interface FixOptions {
  safe: boolean;
  structural: boolean;
  strip_metadata: boolean;
}

export interface AppliedFix {
  code: string;
  count: number;
  message: string;
}

export interface FixResult {
  changed: boolean;
  options: FixOptions;
  applied: AppliedFix[];
  notes: string[];
  before: FormatReport;
  after: FormatReport;
  /** The fixed prompt. */
  text: string;
}

export type WasmSource =
  | URL
  | string
  | Response
  | ArrayBuffer
  | ArrayBufferView
  | WebAssembly.Module;

/** Initialize the module. In Node, calling it without a source loads the
 *  adjacent `rwkv_prompt_format.wasm`; in browsers pass a URL. */
export function init(source?: WasmSource): Promise<void>;

export function check(text: string): Promise<FormatReport>;

export function fix(text: string, flags?: number): Promise<FixResult>;

export function fixText(text: string, flags?: number): Promise<string>;

export function version(): Promise<string>;

export function abiVersion(): Promise<number>;
