"""Jupytext conversion and write plans; Rust owns file changes and conflicts."""

from __future__ import annotations

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

_convert_cache: tuple[tuple[Any, ...], Any] | None = None


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


def _result(notebook: Any, path: Path, config: Any, filename: str | None) -> dict[str, Any]:
    markdown = jupytext.writes(deepcopy(notebook), "md:markdown", config=config)
    return {
        "notebook": notebook,
        "text": markdown[:-1] if markdown.endswith("\n") else markdown,
        "metadata": notebook.metadata,
        "pairs": _pairs(notebook, path, config),
        "config": filename,
        "version": jupytext.__version__,
    }


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


def convert_request(request: dict[str, Any]) -> Any:
    config = _context(request["path"])[1] if request.get("path") else None
    global _convert_cache
    text = str(request.get("text", ""))
    key = None
    if len(text) <= 1_000_000:
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
            key = (str(_path(request["path"])) if request.get("path") else None,
                   jupytext.__version__, settings, text)
    if key is not None and _convert_cache is not None and _convert_cache[0] == key:
        source = _convert_cache[1]
    else:
        _convert_cache = None
        source = jupytext.reads(text, fmt="md:markdown", config=config)
        if key is not None and len(source.cells) <= 4096:
            _convert_cache = (key, source)
    if _convert_cache is not None and source is _convert_cache[1]:
        # Classification replaces metadata; output merging also mutates cells.
        source = deepcopy(source) if request.get("existing") is not None else copy(source)
    if request.get("existing") is not None:
        source = combine_inputs_with_outputs(source, _notebook(request["existing"]), fmt="md:markdown")
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
    elif action in {"pair", "unpair", "sync"}:
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
        if action == "sync" and request.get("source") is not None:
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
    result = _result(notebook, path, config, filename)
    result["writes"] = writes
    if action != "pipe":
        result.pop("notebook")
    return result
