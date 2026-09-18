#!/usr/bin/env python3
"""Small persistent conversion helper for the native notebook engine.

The Rust process owns notebook state and file safety.  This process only calls
the libraries that already define ipynb.nvim's notebook format semantics:
jupytext, nbformat, and the optional rich-output renderers.
"""

from __future__ import annotations

import base64
import json
from pathlib import Path
import sys
from typing import Any

import jupytext
import nbformat
from jupytext.combine import combine_inputs_with_outputs


def _plain(value: Any) -> Any:
    """Turn NotebookNode containers into JSON's ordinary containers."""

    if isinstance(value, dict):
        return {str(key): _plain(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [_plain(item) for item in value]
    return value


def _read(path: str) -> Any:
    # Explicitly selecting ipynb keeps this helper independent of filename
    # extensions and preserves attachments, ids, metadata, and outputs.
    return jupytext.read(path, fmt="ipynb", as_version=nbformat.NO_CONVERT)


def _read_request(request: dict[str, Any]) -> dict[str, Any]:
    path = Path(str(request.get("path", "")))
    template = Path(str(request.get("template", "")))
    source = path if path.is_file() else template
    if not source.is_file():
        raise FileNotFoundError(f"notebook or template not found: {source or path}")

    notebook = _read(str(source))
    markdown = jupytext.writes(notebook, "md:markdown")
    if markdown.endswith("\n"):
        markdown = markdown[:-1]
    return {
        "text": markdown,
        "metadata": _plain(notebook.metadata),
        "notebook": _plain(notebook),
    }


def _convert_request(request: dict[str, Any]) -> Any:
    source = jupytext.reads(str(request.get("text", "")), fmt="md:markdown")
    existing = request.get("existing")
    if existing is not None:
        # combine_inputs_with_outputs requires NotebookNode attributes rather
        # than plain dictionaries. nbformat.from_dict converts recursively.
        output_notebook = nbformat.from_dict(existing)
        source = combine_inputs_with_outputs(source, output_notebook, fmt="md:markdown")
    return _plain(source)


def _text(value: Any) -> str:
    if isinstance(value, list):
        return "".join(str(item) for item in value)
    return str(value)


def _image_bytes(value: Any) -> bytes:
    if isinstance(value, list):
        value = "".join(str(item) for item in value)
    if isinstance(value, bytes):
        return value
    return base64.b64decode(str(value).encode("ascii"), validate=True)


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

    if mime.startswith("image/"):
        target.write_bytes(_image_bytes(data))
        return {"path": str(target), "mime": mime}

    raise ValueError(f"no native renderer for MIME type {mime!r}")


def _dispatch(request: Any) -> Any:
    if not isinstance(request, dict):
        raise TypeError("request must be a JSON object")
    operation = request.get("op")
    if operation == "read":
        return _read_request(request)
    if operation == "convert":
        return _convert_request(request)
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
