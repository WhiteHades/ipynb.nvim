"""Optional Rust-backed JSON decoding with stdlib-compatible fallback."""

from __future__ import annotations

import json
import os
from typing import Any

try:
    import jiter
except ImportError:  # pragma: no cover - exercised by installations without the optional extra
    jiter = None  # type: ignore[assignment]


class NotebookJSONDecoder(json.JSONDecoder):
    """Decode notebook JSON with jiter when no stdlib hooks are requested."""

    def __init__(self, *args: Any, **kwargs: Any) -> None:
        self._use_jiter = jiter is not None and os.environ.get("IPYNB_DISABLE_RUST") != "1"
        self._use_jiter &= not any(
            kwargs.get(name) is not None
            for name in ("object_hook", "object_pairs_hook", "parse_float", "parse_int", "parse_constant")
        )
        self._use_jiter &= kwargs.get("strict", True) is True
        super().__init__(*args, **kwargs)

    def decode(self, s: str, _w=json.decoder.WHITESPACE.match) -> Any:
        if not self._use_jiter:
            return super().decode(s)
        try:
            return jiter.from_json(s.encode("utf-8"), cache_mode="keys")
        except (ValueError, TypeError):
            return super().decode(s)
