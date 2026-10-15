#!/usr/bin/env python3
"""Small persistent conversion helper for the native notebook engine.

The Rust process owns notebook state and file safety.  This process only calls
the libraries that already define ipynb.nvim's notebook format semantics:
jupytext, nbformat, and the optional rich-output renderers.
"""

from __future__ import annotations

import json
import os
from pathlib import Path
import sys
from tempfile import TemporaryDirectory
from typing import Any

# Jupytext probes Pandoc while importing its format registry. Ordinary notebook
# reads must not execute optional tools from PATH.
with TemporaryDirectory(dir=os.environ.get("IPYNB_CONVERTER_TMPDIR")) as import_path:
    original_path = os.environ.get("PATH")
    optional_managers = ("jupytext.sync_contentsmanager", "jupytext.async_contentsmanager")
    missing = object()
    previous_modules = {name: sys.modules.get(name, missing) for name in optional_managers}
    try:
        os.environ["PATH"] = import_path
        for name in optional_managers:
            sys.modules[name] = None
        import jupytext_adapter
    finally:
        if original_path is None:
            os.environ.pop("PATH", None)
        else:
            os.environ["PATH"] = original_path
        for name, module in previous_modules.items():
            if module is missing:
                sys.modules.pop(name, None)
            else:
                sys.modules[name] = module


def _text(value: Any) -> str:
    if isinstance(value, list):
        return "".join(str(item) for item in value)
    return str(value)


def _render_request(request: dict[str, Any]) -> dict[str, str]:
    """Render one optional rich-output MIME value to the requested path.

    Imports stay inside each branch so Plotly, Kaleido, pnglatex, and
    CairoSVG remain optional. A missing package or renderer error is returned
    to Rust as a failed request; the caller can then use text or another MIME
    value as its fallback.
    """

    mime = str(request.get("mime", ""))
    target = Path(str(request.get("path", "")))
    if not str(target):
        raise ValueError("render path is required")
    target.parent.mkdir(parents=True, exist_ok=True)
    data = request.get("data")

    if mime == "image/svg+xml":
        import cairosvg

        cairosvg.svg2png(bytestring=_text(data).encode("utf-8"), write_to=str(target))
        return {"path": str(target), "mime": "image/png"}

    if mime == "application/vnd.plotly.v1+json":
        from plotly.io import from_json

        import kaleido  # noqa: F401  # Import is the optional dependency check.

        figure = from_json(json.dumps(data, ensure_ascii=False))
        figure.write_image(str(target), engine="kaleido")
        return {"path": str(target), "mime": "image/png"}

    if mime == "text/latex":
        from pnglatex import pnglatex

        target.touch()
        pnglatex(_text(data), str(target))
        return {"path": str(target), "mime": "image/png"}

    raise ValueError(f"no native renderer for MIME type {mime!r}")


def _dispatch(request: Any) -> Any:
    if not isinstance(request, dict):
        raise TypeError("request must be a JSON object")
    operation = request.get("op")
    if operation == "read":
        return jupytext_adapter.read_request(request)
    if operation == "convert":
        return jupytext_adapter.convert_request(request)
    if operation == "jupytext_info":
        return jupytext_adapter.info_request(request)
    if operation == "jupytext_action":
        return jupytext_adapter.action_request(request)
    if operation == "render":
        return _render_request(request)
    if operation == "shutdown":
        raise EOFError
    raise ValueError(f"unknown converter operation: {operation!r}")


def serve() -> None:
    for line in sys.stdin:
        try:
            request = json.loads(line)
            result = _dispatch(request)
        except EOFError:
            return
        except Exception as error:  # The Rust side receives all request errors.
            response = {"ok": False, "error": f"{type(error).__name__}: {error}"}
        else:
            response = {"ok": True, "result": result}
        try:
            sys.stdout.write(json.dumps(response, ensure_ascii=False, separators=(",", ":")) + "\n")
            sys.stdout.flush()
        except BrokenPipeError:
            return


if __name__ == "__main__":
    serve()
