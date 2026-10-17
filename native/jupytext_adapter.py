"""Jupytext conversion and write plans; Rust owns file changes and conflicts."""

from __future__ import annotations

from bisect import bisect_right
from collections import OrderedDict
from contextlib import redirect_stdout
from copy import copy, deepcopy
from hashlib import sha256
import json
import os
from pathlib import Path
import sys
from tempfile import TemporaryDirectory
from typing import Any

import jupytext
import nbformat
from packaging.version import Version
from jupytext.combine import combine_inputs_with_outputs
from jupytext.config import find_jupytext_configuration_file, load_jupytext_configuration_file, notebook_formats
from jupytext.formats import (
    JUPYTEXT_FORMATS,
    check_file_version,
    format_name_for_ext,
    get_format_implementation,
    guess_format,
    long_form_multiple_formats,
    long_form_one_format,
    read_format_from_metadata,
    rearrange_jupytext_metadata,
    short_form_multiple_formats,
)
from jupytext.paired_paths import base_path, find_base_path_and_format, paired_paths
from jupytext.sync_pairs import write_pair

_convert_cache: OrderedDict[tuple[Any, ...], Any] = OrderedDict()


def _path(value: Any) -> Path:
    if not isinstance(value, str) or not value:
        raise ValueError("notebook path is required")
    return Path(os.path.abspath(value))


def _context(value: Any) -> tuple[Path, Any, str | None]:
    if Version(jupytext.__version__) < Version("1.19.6"):
        raise RuntimeError("ipynb.nvim needs Jupytext 1.19.6 or newer. Run :IpynbInstall, or update your custom helper environment.")
    path = _path(value)
    filename = find_jupytext_configuration_file(path)
    if filename and Path(filename).suffix == ".py":
        raise ValueError(
            f"Executable Jupytext config is disabled: {filename}. "
            "Move its settings to jupytext.toml or [tool.jupytext] in pyproject.toml."
        )
    config = load_jupytext_configuration_file(filename) if filename else None
    return path, config, filename


def _notebook(value: Any) -> Any:
    if not isinstance(value, dict):
        raise TypeError("notebook must be a JSON object")
    # Normalize disk line arrays without reparsing the common v4 representation.
    if value.get("nbformat") == 4:
        from nbformat.v4.nbjson import JSONReader

        return JSONReader().to_notebook(value)
    return nbformat.reads(json.dumps(value), as_version=4)


def _read(request: dict[str, Any], path: Path, config: Any) -> tuple[Any, str | None, bool]:
    data = path.read_bytes() if path.is_file() else None
    source = data
    if not source:
        template = Path(str(request.get("template", "")))
        if not template.is_file():
            raise FileNotFoundError(f"notebook or template not found: {path}")
        source = template.read_bytes()
    content = source.decode("utf-8")
    recovered = False
    try:
        notebook = jupytext.reads(content, fmt="ipynb", as_version=4, config=config)
    except nbformat.reader.NotJSONError:
        if (not data or path.suffix != ".ipynb"
                or content.lstrip("\ufeff").splitlines()[:2] != ["---", "jupyter:"]
                or read_format_from_metadata(content, ".md") != "markdown"):
            raise
        notebook = jupytext.reads(content, fmt="md:markdown", as_version=4, config=config)
        nbformat.validate(notebook)
        recovered = True
    rearrange_jupytext_metadata(notebook.metadata)
    return notebook, sha256(data).hexdigest() if data is not None else None, recovered


def _pairs(notebook: Any, path: Path, config: Any) -> list[dict[str, Any]]:
    formats = notebook_formats(notebook, config, str(path))
    _, fmt = find_base_path_and_format(str(path), formats)
    return [{"path": str(_path(name)), "fmt": fmt} for name, fmt in paired_paths(str(path), fmt, formats)]


def _result(
    notebook: Any, path: Path, config: Any, filename: str | None, *, include_text: bool = True
) -> dict[str, Any]:
    result = {
        "notebook": notebook,
        "metadata": notebook.metadata,
        "pairs": _pairs(notebook, path, config),
        "config": filename,
        "version": jupytext.__version__,
    }
    if include_text:
        markdown = jupytext.writes(deepcopy(notebook), "md:markdown", config=config)
        result["text"] = markdown[:-1] if markdown.endswith("\n") else markdown
    return result


def read_request(request: dict[str, Any]) -> dict[str, Any]:
    path, config, filename = _context(request.get("path"))
    notebook, digest, recovered = _read(request, path, config)
    result = _result(notebook, path, config, filename)
    if recovered:
        result["recovery_digest"] = digest
    divergent = []
    for pair in result["pairs"]:
        if pair["path"] == str(path):
            pair["digest"] = digest
            continue
        try:
            data = Path(pair["path"]).read_bytes()
        except FileNotFoundError:
            pair["digest"] = None
            continue
        pair["digest"] = sha256(data).hexdigest()
        fmt = pair["fmt"]
        try:
            # Opening a notebook must not launch optional external converters.
            content = data.decode("utf-8")
            ext = fmt["extension"]
            external = {"quarto", "pandoc", "marimo"}
            reader = None
            writer = fmt.get("format_name")
            if ext != ".ipynb":
                reader = read_format_from_metadata(content, ext) or fmt.get("format_name") or guess_format(content, ext)[0]
                writer = writer or format_name_for_ext(notebook.metadata, ext, explicit_default=False)
            if ext == ".qmd" or reader in external or writer in external:
                divergent.append(pair["path"])
                continue
            inputs = jupytext.reads(content, fmt=fmt, as_version=4, config=config)
            if ext != ".ipynb":
                check_file_version(inputs, pair["path"], str(path))
            if jupytext.writes(inputs, fmt, config=config) != jupytext.writes(deepcopy(notebook), fmt, config=config):
                divergent.append(pair["path"])
        except Exception:
            divergent.append(pair["path"])
    result["divergent"] = divergent
    return result


def _read_positions(text: str, config: Any) -> tuple[Any, list[tuple[tuple[int, int], tuple[int, int]]]]:
    from jupytext.jupytext import TextNotebookConverter

    reader = TextNotebookConverter(
        {"extension": ".md", "format_name": read_format_from_metadata(text, ".md") or "markdown"}, config
    )
    implementation = copy(reader.implementation)
    if implementation.cell_reader_class is None:
        raise ValueError("Notebook text format does not support editor cell positions")
    ranges = []
    points = []
    row, col = 0, 0
    for line in text.splitlines(keepends=True):
        points.append((row, col))
        if line.endswith("\n"):
            row, col = row + 1, 0
        else:
            col += len(line.encode("utf-8"))
    line_count = len(points)
    points.append((row, col))

    class PositionedReader(implementation.cell_reader_class):
        def read(self, lines):
            start = line_count - len(lines)
            cell, consumed = super().read(lines)
            if cell.cell_type == "code":
                first = start + int(self.explicit_soc)
                ranges.append((points[first], points[first + len(self.org_content)]))
            return cell, consumed

    implementation.cell_reader_class = PositionedReader
    reader.implementation = implementation
    notebook = reader.reads(text)
    if len(ranges) != sum(cell.cell_type == "code" for cell in notebook.cells):
        raise ValueError("Notebook parser returned inconsistent editor cell positions")
    return notebook, ranges


def convert_request(request: dict[str, Any]) -> Any:
    config = _context(request["path"])[1] if request.get("path") else None
    cache = request.get("cache") is not False
    transient = request.get("cache") == "positions"
    positioned = "positions" in request
    text = str(request.get("text", ""))
    key = None
    if cache and len(text) <= 1_000_000:
        try:
            traits = config.traits(config=True) if config is not None else None
            settings = (
                json.dumps(
                    {name: getattr(config, name) for name in traits},
                    allow_nan=False,
                    sort_keys=True,
                )
                if config is not None else None
            )
        except (TypeError, ValueError):
            pass
        else:
            key = (transient, positioned, str(_path(request["path"])) if request.get("path") else None,
                   jupytext.__version__, settings, text)
    if key is not None and key in _convert_cache:
        source, ranges = _convert_cache[key]
        _convert_cache.move_to_end(key)
    else:
        source, ranges = (_read_positions(text, config) if positioned
                          else (jupytext.reads(text, fmt="md:markdown", config=config), None))
        if cache and key is not None and len(source.cells) <= 4096:
            if not transient:
                for cached_key in list(_convert_cache):
                    if not cached_key[0]:
                        del _convert_cache[cached_key]
            _convert_cache[key] = source, ranges
            while (len(_convert_cache) > 3
                   or sum(len(entry[-1]) for entry in _convert_cache) > 1_000_000
                   or sum(len(entry[0].cells) for entry in _convert_cache.values()) > 4096):
                oldest_transient = next((cached_key for cached_key in _convert_cache if cached_key[0]), None)
                if oldest_transient is None:
                    _convert_cache.popitem(last=False)
                else:
                    del _convert_cache[oldest_transient]
    if positioned:
        first_points = [first for first, _ in ranges]
        positions = {}
        for identity, (row, col) in request["positions"]:
            point = row, col
            ordinal = bisect_right(first_points, point) - 1
            if ordinal < 0:
                continue
            first, stop = ranges[ordinal]
            if point >= stop and not point == first == stop:
                continue
            if identity in positions and positions[identity] != ordinal:
                raise ValueError("Ambiguous notebook output position; file was not changed")
            positions[identity] = ordinal
        return {"sources": [cell.source for cell in source.cells if cell.cell_type == "code"],
                "positions": list(positions.items())}
    if key is not None and key in _convert_cache and _convert_cache[key][0] is source:
        # Classification replaces metadata; output merging also mutates cells.
        source = deepcopy(source) if request.get("existing") is not None else copy(source)
    if request.get("existing") is not None:
        existing = _notebook(request["existing"])
        inputs = source
        source = combine_inputs_with_outputs(inputs, existing, fmt="md:markdown")
        originals = {cell.get("id"): cell for cell in existing.cells if cell.cell_type == "code"}
        anchored, reserved = set(), set()
        ordinal = 0
        for index, cell in enumerate(inputs.cells):
            if cell.cell_type != "code":
                continue
            identity = request.get("identities", {}).get(str(ordinal))
            ordinal += 1
            if identity is None or identity["id"] not in originals:
                continue
            if cell.source != identity["source"] or identity["id"] in reserved:
                raise ValueError("Notebook cell identity does not match its editor position")
            left, right = copy(inputs), copy(existing)
            previous = copy(originals[identity["id"]])
            previous.source = cell.source
            left.cells, right.cells = [cell], [previous]
            source.cells[index] = combine_inputs_with_outputs(left, right, fmt="md:markdown").cells[0]
            anchored.add(index)
            reserved.add(identity["id"])
        for index, cell in enumerate(source.cells):
            if index not in anchored and cell.get("id") in reserved:
                source.cells[index] = deepcopy(inputs.cells[index])
    if request.get("path"):
        from jupytext.jupytext import drop_text_representation_metadata

        source.metadata = drop_text_representation_metadata(source).metadata
    return source


def info_request(request: dict[str, Any]) -> dict[str, Any]:
    path, config, filename = _context(request.get("path"))
    notebook = _notebook(request["notebook"]) if request.get("notebook") is not None else _read(request, path, config)[0]
    # These checks can launch optional tools. Ordinary reads never need them.
    from jupytext.marimo import MARIMO_MIN_VERSION, is_marimo_available
    from jupytext.myst import is_myst_available
    from jupytext.pandoc import is_pandoc_available
    from jupytext.quarto import QUARTO_MIN_VERSION, is_quarto_available

    optional = {
        "marimo": (is_marimo_available(), f"marimo>={MARIMO_MIN_VERSION}"),
        "myst": (is_myst_available(), "markdown-it-py and mdit-py-plugins"),
        "pandoc": (is_pandoc_available(), "pandoc>=2.7.2"),
        "quarto": (is_quarto_available(), f"quarto>={QUARTO_MIN_VERSION}"),
    }
    formats = [{"format": "ipynb", "extension": ".ipynb", "name": "ipynb", "available": True, "requires": None}]
    seen = {"ipynb"}
    for description in JUPYTEXT_FORMATS:
        spec = f"{description.extension[1:]}:{description.format_name}"
        if spec in seen:
            continue
        seen.add(spec)
        available, requires = optional.get(description.format_name, (True, None))
        formats.append({"format": spec, "extension": description.extension, "name": description.format_name,
                        "available": available, "requires": requires})
    return {"version": jupytext.__version__, "config": filename,
            "pairs": _pairs(notebook, path, config), "formats": formats}


def _prepare_external_format(fmt: dict[str, Any]) -> None:
    if fmt.get("format_name") == "pandoc":
        from jupytext.pandoc import pandoc_version

        get_format_implementation(".md", "pandoc").current_version_number = pandoc_version()


def _format(spec: Any, notebook: Any) -> dict[str, Any]:
    if not isinstance(spec, (str, dict)) or not spec:
        raise ValueError("a Jupytext format is required")
    fmt = long_form_one_format(deepcopy(spec), notebook.metadata)
    _prepare_external_format(fmt)
    if fmt["extension"] != ".ipynb":
        get_format_implementation(fmt["extension"], fmt.get("format_name"))
    return fmt


def _write(notebook: Any, path: str, fmt: dict[str, Any], config: Any) -> dict[str, Any]:
    _prepare_external_format(fmt)
    if fmt["extension"] == ".ipynb":
        return {"path": path, "notebook": deepcopy(notebook)}
    content = jupytext.writes(deepcopy(notebook), fmt=deepcopy(fmt), config=config)
    return {"path": path, "content": content if content.endswith("\n") else content + "\n"}


def _run_pipe(notebook: Any, command: Any, fmt: Any, path: Path, update: bool) -> Any:
    if not isinstance(command, str) or not command.strip():
        raise ValueError("pipe/check requires an explicit command")
    from jupytext.cli import pipe_notebook

    cwd = Path.cwd()
    stdout = os.dup(1)
    try:
        # Subprocesses inherit fd 1 even when Python's sys.stdout is redirected.
        sys.stdout.flush()
        os.dup2(2, 1)
        os.chdir(path.parent)
        with TemporaryDirectory(dir=os.environ.get("IPYNB_CONVERTER_TMPDIR")) as directory, redirect_stdout(sys.stderr):
            return pipe_notebook(deepcopy(notebook), command, fmt=fmt, update=update,
                                 quiet=True, prefix=path.stem, directory=directory)
    except SystemExit as error:
        raise RuntimeError(f"Jupytext command exited with status {error.code}") from None
    finally:
        os.dup2(stdout, 1)
        os.close(stdout)
        os.chdir(cwd)


def action_request(request: dict[str, Any]) -> dict[str, Any]:
    path, config, filename = _context(request.get("path"))
    if path.suffix != ".ipynb":
        raise ValueError("Jupytext actions require the current .ipynb notebook path")
    notebook = _notebook(request.get("notebook"))
    action = request.get("action")
    writes = []
    if action == "export":
        target = request.get("target")
        if not isinstance(target, str) or not Path(target).is_absolute():
            raise ValueError("export requires an absolute target path")
        target = str(_path(target))
        fmt = _format(request.get("fmt"), notebook)
        base_path(target, fmt)
        paired_paths(target, fmt, [fmt])
        writes.append(_write(notebook, target, fmt, config))
    elif action in {"pipe", "check"}:
        fmt = _format(request.get("fmt", "py:percent"), notebook)
        if config is not None:
            config.set_default_format_options(fmt)
        transformed = _run_pipe(notebook, request.get("command"), fmt, path, update=action == "pipe")
        if action == "pipe":
            notebook = transformed
    elif action in {"pair", "unpair", "sync", "save"}:
        if action in {"pair", "unpair"}:
            spec = "ipynb" if action == "unpair" else request.get("formats")
            if not isinstance(spec, str) or not spec.strip():
                raise ValueError("pair requires a nonempty formats specification including ipynb")
            formats = [_format(fmt, notebook) for fmt in long_form_multiple_formats(spec, notebook.metadata)]
            notebook.metadata.setdefault("jupytext", {})["formats"] = short_form_multiple_formats(formats)
        pairs = _pairs(notebook, path, config)
        ipynb_paths = [pair["path"] for pair in pairs if pair["fmt"]["extension"] == ".ipynb"]
        if ipynb_paths != [str(path)]:
            raise ValueError("pairing must contain exactly the current .ipynb notebook")
        if action in {"sync", "save"} and request.get("source") is not None:
            source = str(_path(request["source"]))
            selected = next((pair for pair in pairs if pair["path"] == source), None)
            if selected is None:
                raise ValueError("sync source must belong to the notebook's resolved pairs")
            _prepare_external_format(selected["fmt"])
            inputs = jupytext.read(source, fmt=selected["fmt"], as_version=4, config=config)
            if selected["fmt"]["extension"] != ".ipynb":
                check_file_version(inputs, source, str(path))
            notebook = combine_inputs_with_outputs(inputs, notebook, fmt=selected["fmt"])
            if _pairs(notebook, path, config) != pairs:
                raise ValueError("sync source changes pairing; use the pair action to select new destinations")

        def collect(name: str, fmt: dict[str, Any], **_: Any) -> dict[str, bool]:
            writes.append(_write(notebook, str(_path(name)), fmt, config))
            return {"modified": True}

        write_pair(str(path), [pair["fmt"] for pair in pairs], collect)
    else:
        raise ValueError(f"unknown Jupytext action: {action!r}")
    result = _result(notebook, path, config, filename, include_text=action != "save")
    result["writes"] = writes
    if action != "pipe":
        result.pop("notebook")
    return result
