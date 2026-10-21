export interface SeverityMap {
  error: 'error';
  warn: 'warn';
  info: 'info';
}

export type Severity = 'error' | 'warn' | 'info';

export interface Finding {
  /** Stable rule code, e.g. MULTIPLE_SYSTEM, BLANK_LINE_IN_CONTENT. */
  code: string;
  severity: Severity;
  /** 1-based line; 0 means a whole-text issue. */
  line: number;
  message: string;
  evidence: string;
}

export interface GenerationPoint {
  line: number;
  role: string;
  /** true when text is already seeded after the label (e.g. `Assistant: {"answer": `). */
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

export const FIX_SAFE: number;
export const FIX_STRUCTURAL: number;
export const FIX_STRIP_METADATA: number;
export const FIX_ALL: number;

export function version(): string;
export function check(text: string): FormatReport;
export function fix(text: string, flags?: number): FixResult;
export function fixText(text: string, flags?: number): string | null;
