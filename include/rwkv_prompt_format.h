/*
 * rwkv_prompt_format.h — RWKV G1/G1x prompt format checker and auto-fixer.
 *
 * Pure text in, JSON out. No model, no GPU, no I/O. All functions are
 * re-entrant and thread-safe.
 *
 * Strings are UTF-8 and NUL-terminated. Every `char *` returned by this
 * library must be released with rwkv_pf_free_string(); do NOT call free()
 * on it (the allocator may differ across CRT boundaries).
 *
 * JSON APIs never return NULL for a valid call: on bad input (NULL pointer,
 * invalid UTF-8, internal panic) they return {"error":"..."} so wrappers can
 * use one code path. rwkv_pf_fix_text is the exception and returns NULL.
 *
 * Build: cargo build --release
 * Artifacts: target/release/rwkv_prompt_format.dll (Windows)
 *            target/release/librwkv_prompt_format.so (Linux)
 *            target/release/librwkv_prompt_format.dylib (macOS)
 */

#ifndef RWKV_PROMPT_FORMAT_H
#define RWKV_PROMPT_FORMAT_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define RWKV_PF_VERSION "0.1.0"

/* Fix flags (bitmask; combine with |). */
#define RWKV_PF_FIX_SAFE 0x1u           /* labels / case / colons / spacing / blank lines / think / Tools / BOM / CRLF */
#define RWKV_PF_FIX_STRUCTURAL 0x2u     /* convert ChatML / Llama3 / JSONL / Markdown, merge blocks, add generation point */
#define RWKV_PF_FIX_STRIP_METADATA 0x4u /* drop "[seg 12 · user] ..." memory prefixes (content edit, off by default) */

/* Returns the library version ("0.1.0"). Static storage: do not free. */
const char *rwkv_pf_version(void);

/*
 * Check a prompt. Returns a FormatReport JSON object:
 *   {profile, format, ok, error_count, warning_count, info_count,
 *    generation_point, stats, findings[]}
 * Release the result with rwkv_pf_free_string().
 */
char *rwkv_pf_check(const char *text_utf8);

/*
 * Fix a prompt. Returns a FixResult JSON object:
 *   {changed, options, applied[], notes[], before, after, text}
 * `text` is the fixed prompt; `after` is the checker's report on it.
 * Release the result with rwkv_pf_free_string().
 */
char *rwkv_pf_fix(const char *text_utf8, uint32_t flags);

/*
 * Fix a prompt and return only the fixed text. Returns NULL on failure
 * (NULL input, invalid UTF-8, internal panic).
 * Release the result with rwkv_pf_free_string().
 */
char *rwkv_pf_fix_text(const char *text_utf8, uint32_t flags);

/* Release a string returned by this library. NULL is a no-op. */
void rwkv_pf_free_string(char *ptr);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* RWKV_PROMPT_FORMAT_H */
