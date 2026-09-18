"""Fast local notebook conversion for the jupytext bridge."""

from __future__ import annotations

import os
from pathlib import Path
from typing import Any

import jupytext
from jupytext.combine import combine_inputs_with_outputs

from ipynb_runtime.ipynb import write_notebook_file
from ipynb_runtime.json_decoder import NotebookJSONDecoder


class NotebookIOError(RuntimeError):
    """A notebook could not be read or safely written."""


def _without_trailing_newline(text: str) -> str:
    return text[:-1] if text.endswith("\n") else text


def _read(path: str) -> Any:
    return jupytext.reads(
        Path(path).read_text(encoding="utf-8"), fmt="ipynb", cls=NotebookJSONDecoder
    )


def _mtime(path: str) -> tuple[int, int] | None:
    try:
        value = os.stat(path).st_mtime_ns
    except FileNotFoundError:
        return None
    return value // 1_000_000_000, value % 1_000_000_000


def _expected_mtime(value: Any) -> tuple[int, int] | None:
    if value is None:
        return None
    if isinstance(value, dict) and "sec" in value and "nsec" in value:
        return int(value["sec"]), int(value["nsec"])
    raise NotebookIOError(f"invalid expected notebook mtime: {value!r}")


def _check_mtime(path: str, expected: tuple[int, int] | None) -> None:
    if expected is not None and _mtime(path) != expected:
        raise NotebookIOError("notebook changed on disk since it was opened")


def read_notebook(path: str, template: str) -> dict[str, Any]:
    """Return markdown text and metadata for a local notebook path."""
    source = path if os.path.isfile(path) else template
    if not source or not os.path.isfile(source):
        raise NotebookIOError(f"notebook template not found: {source or path}")
    try:
        notebook = _read(source)
        text = jupytext.writes(notebook, "md:markdown")
    except Exception as error:
        raise NotebookIOError(f"could not read notebook {source}: {error}") from error
    return {"text": _without_trailing_newline(text), "metadata": dict(notebook.metadata)}


def write_notebook(path: str, text: str, expected_mtime: Any = None) -> dict[str, Any]:
    """Convert markdown text and atomically write a notebook.

    Existing notebooks supply outputs, cell ids, and notebook metadata through
    jupytext's ``--update`` equivalent.  ``expected_mtime`` is checked before
    reading and again immediately before the atomic replacement.
    """
    target = os.path.realpath(path)
    expected = _expected_mtime(expected_mtime)
    _check_mtime(target, expected)
    try:
        source = jupytext.reads(text, fmt="md:markdown")
        existing = _read(target) if os.path.isfile(target) else None
        notebook = (
            combine_inputs_with_outputs(source, existing, fmt="md:markdown")
            if existing is not None
            else source
        )
    except NotebookIOError:
        raise
    except Exception as error:
        raise NotebookIOError(f"could not convert notebook {path}: {error}") from error

    _check_mtime(target, expected)
    try:
        write_notebook_file(notebook, target)
        value = os.stat(target).st_mtime_ns
    except Exception as error:
        raise NotebookIOError(f"could not write notebook {path}: {error}") from error
    return {
        "mtime": {
            "sec": value // 1_000_000_000,
            "nsec": value % 1_000_000_000,
        }
    }
