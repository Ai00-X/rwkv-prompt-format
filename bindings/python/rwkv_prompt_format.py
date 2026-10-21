"""RWKV G1/G1x prompt format checker and auto-fixer (ctypes binding).

Usage:
    from rwkv_prompt_format import check, fix, fix_text, FIX_SAFE, FIX_STRUCTURAL

    report = check(text)
    if not report["ok"]:
        result = fix(text)
        text = result["text"]

Library discovery order:
    1. RWKV_PROMPT_FORMAT_LIB (full path to the shared library)
    2. <repo>/target/release/{rwkv_prompt_format.dll, librwkv_prompt_format.so, librwkv_prompt_format.dylib}
    3. the system loader search path

Pure Python 3.8+, no third-party dependencies.
"""

from __future__ import annotations

import ctypes
import json
import os
import sys
from ctypes import c_char_p, c_uint32, c_void_p
from pathlib import Path
from typing import Any, Dict, Optional

FIX_SAFE = 0x1
FIX_STRUCTURAL = 0x2
FIX_STRIP_METADATA = 0x4
FIX_ALL = FIX_SAFE | FIX_STRUCTURAL

if sys.platform == "win32":
    _LIB_NAMES = ["rwkv_prompt_format.dll"]
elif sys.platform == "darwin":
    _LIB_NAMES = ["librwkv_prompt_format.dylib"]
else:
    _LIB_NAMES = ["librwkv_prompt_format.so"]

_REPO_ROOT = Path(__file__).resolve().parents[2]


def _candidate_paths():
    env = os.environ.get("RWKV_PROMPT_FORMAT_LIB")
    if env:
        yield Path(env)
    for name in _LIB_NAMES:
        yield _REPO_ROOT / "target" / "release" / name
    yield Path(_LIB_NAMES[0])


def _load() -> ctypes.CDLL:
    errors = []
    for path in _candidate_paths():
        try:
            lib = ctypes.CDLL(str(path))
        except OSError as exc:
            errors.append(f"{path}: {exc}")
            continue
        lib.rwkv_pf_version.restype = c_char_p
        lib.rwkv_pf_version.argtypes = []
        lib.rwkv_pf_check.restype = c_void_p
        lib.rwkv_pf_check.argtypes = [c_char_p]
        lib.rwkv_pf_fix.restype = c_void_p
        lib.rwkv_pf_fix.argtypes = [c_char_p, c_uint32]
        lib.rwkv_pf_fix_text.restype = c_void_p
        lib.rwkv_pf_fix_text.argtypes = [c_char_p, c_uint32]
        lib.rwkv_pf_free_string.restype = None
        lib.rwkv_pf_free_string.argtypes = [c_void_p]
        return lib
    raise OSError(
        "cannot load rwkv_prompt_format; set RWKV_PROMPT_FORMAT_LIB.\n"
        + "\n".join(errors)
    )


_lib = _load()


def _take(ptr: Optional[int]) -> Optional[str]:
    if not ptr:
        return None
    try:
        return ctypes.string_at(ptr).decode("utf-8")
    finally:
        _lib.rwkv_pf_free_string(ptr)


def version() -> str:
    """Library version, e.g. "0.1.0"."""
    return _lib.rwkv_pf_version().decode("utf-8")


def _decode(text: str) -> bytes:
    return text.encode("utf-8")


def check(text: str) -> Dict[str, Any]:
    """Check a prompt. Returns the parsed FormatReport dict."""
    raw = _take(_lib.rwkv_pf_check(_decode(text)))
    return json.loads(raw if raw is not None else '{"error":"null result"}')


def fix(text: str, flags: int = FIX_ALL) -> Dict[str, Any]:
    """Fix a prompt. Returns the parsed FixResult dict (contains "text")."""
    raw = _take(_lib.rwkv_pf_fix(_decode(text), flags))
    return json.loads(raw if raw is not None else '{"error":"null result"}')


def fix_text(text: str, flags: int = FIX_ALL) -> Optional[str]:
    """Fix a prompt and return only the fixed text (None on failure)."""
    return _take(_lib.rwkv_pf_fix_text(_decode(text), flags))


if __name__ == "__main__":
    source = sys.stdin.read()
    result = fix(source)
    sys.stdout.write(result.get("text", ""))
    summary = {
        "changed": result.get("changed"),
        "applied": result.get("applied"),
        "notes": result.get("notes"),
        "error_count": result.get("after", {}).get("error_count"),
    }
    print(json.dumps(summary, ensure_ascii=False), file=sys.stderr)
