import json
import os
import stat
import tempfile
from typing import Type, Optional, Dict, Any

from pynvim import Nvim

from pynvim.api import Buffer
from ipynb_runtime.code_cell import CodeCell
from ipynb_runtime.position import DynamicPosition

from ipynb_runtime.utils import IpynbException
from ipynb_runtime.options import IpynbOptions
from ipynb_runtime.outputchunks import (
    ErrorOutputChunk,
    OutputStatus,
    Output,
    TextOutputChunk,
    to_outputchunk,
)
from ipynb_runtime.outputbuffer import OutputBuffer
from ipynb_runtime.moltenbuffer import IpynbKernel


class IpynbIOError(Exception):
    @classmethod
    def assert_has_key(
        cls, data: Dict[str, Any], key: str, type_: Optional[Type[Any]] = None
    ) -> Any:
        if key not in data:
            raise cls(f"Missing key: {key}")
        value = data[key]
        if type_ is not None and not isinstance(value, type_):
            raise cls(
                f"Incorrect type for key '{key}': expected {type_.__name__}, \
                got {type(value).__name__}"
            )
        return value


def get_default_save_file(options: IpynbOptions, buffer: Buffer) -> str:
    # XXX: this is string containment checking. Beware.
    if "nofile" in buffer.options["buftype"]:
        raise IpynbException("Buffer does not correspond to a file")

    mangled_name = buffer.name.replace("%", "%%").replace("/", "%")

    return os.path.join(options.save_path, mangled_name + ".json")


def write_save_file(path: str, data: Dict[str, Any]) -> None:
    """Write saved kernel state atomically, including for relative paths."""
    target = os.path.realpath(path)
    directory = os.path.dirname(os.path.abspath(target))
    try:
        mode = stat.S_IMODE(os.stat(target).st_mode)
    except FileNotFoundError:
        mode = None
    os.makedirs(directory, exist_ok=True)
    fd, temporary_path = tempfile.mkstemp(
        prefix=f".{os.path.basename(target)}.", dir=directory, text=True
    )
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as file:
            json.dump(data, file)
            file.flush()
            os.fsync(file.fileno())
        if mode is not None:
            os.chmod(temporary_path, mode)
        os.replace(temporary_path, target)
    except BaseException:
        try:
            os.unlink(temporary_path)
        except FileNotFoundError:
            pass
        raise


def _serialize_chunk(chunk: Any) -> Dict[str, Any]:
    """Convert an output chunk into the persisted, JSON-compatible form."""
    if isinstance(chunk, ErrorOutputChunk):
        return {
            "data": {},
            "metadata": {},
            "output_type": "error",
            "extras": dict(chunk.extras),
        }

    data = chunk.jupyter_data
    if data is None and isinstance(chunk, TextOutputChunk):
        data = {"text/plain": chunk.text}

    return {
        "data": data or {},
        "metadata": chunk.jupyter_metadata or {},
        "output_type": chunk.output_type,
        "extras": dict(chunk.extras),
    }


def load(nvim: Nvim, moltenbuffer: IpynbKernel, nvim_buffer: Buffer, data: Dict[str, Any]) -> None:
    IpynbIOError.assert_has_key(data, "content_checksum", str)

    if moltenbuffer._get_content_checksum() != data["content_checksum"]:
        raise IpynbIOError("Buffer contents' checksum does not match!")

    IpynbIOError.assert_has_key(data, "cells", list)
    for cell in data["cells"]:
        IpynbIOError.assert_has_key(cell, "span", dict)
        IpynbIOError.assert_has_key(cell["span"], "begin", dict)
        IpynbIOError.assert_has_key(cell["span"]["begin"], "lineno", int)
        IpynbIOError.assert_has_key(cell["span"]["begin"], "colno", int)
        IpynbIOError.assert_has_key(cell["span"], "end", dict)
        IpynbIOError.assert_has_key(cell["span"]["end"], "lineno", int)
        IpynbIOError.assert_has_key(cell["span"]["end"], "colno", int)
        begin_position = DynamicPosition(
            moltenbuffer.nvim,
            moltenbuffer.extmark_namespace,
            nvim_buffer.number,
            cell["span"]["begin"]["lineno"],
            cell["span"]["begin"]["colno"],
        )
        end_position = DynamicPosition(
            moltenbuffer.nvim,
            moltenbuffer.extmark_namespace,
            nvim_buffer.number,
            cell["span"]["end"]["lineno"],
            cell["span"]["end"]["colno"],
            right_gravity=True,
        )
        span = CodeCell(nvim, begin_position, end_position)

        # XXX: do we really want to have the execution count here?
        #      what happens when the counts start to overlap?
        execution_count = IpynbIOError.assert_has_key(cell, "execution_count")
        if execution_count is not None and not isinstance(execution_count, int):
            raise IpynbIOError(
                "Incorrect type for key 'execution_count': expected int or None, "
                f"got {type(execution_count).__name__}"
            )
        output = Output(execution_count)

        IpynbIOError.assert_has_key(cell, "status", int)
        output.status = OutputStatus(cell["status"])

        IpynbIOError.assert_has_key(cell, "success", bool)
        output.success = cell["success"]

        IpynbIOError.assert_has_key(cell, "chunks", list)
        for chunk in cell["chunks"]:
            IpynbIOError.assert_has_key(chunk, "data", dict)
            IpynbIOError.assert_has_key(chunk, "metadata", dict)
            if chunk.get("output_type") == "error":
                extras = chunk.get("extras", {})
                if not isinstance(extras, dict):
                    raise IpynbIOError("Incorrect type for key 'extras': expected dict")
                traceback = extras.get("traceback", [])
                if not isinstance(traceback, list):
                    raise IpynbIOError("Incorrect type for key 'traceback': expected list")
                error_chunk = ErrorOutputChunk(
                    str(extras.get("ename", "Error")),
                    str(extras.get("evalue", "")),
                    [str(line) for line in traceback],
                )
                error_chunk.extras = extras
                output.chunks.append(error_chunk)
            else:
                restored = to_outputchunk(
                    nvim,
                    moltenbuffer.runtime._alloc_file,
                    chunk["data"],
                    chunk["metadata"],
                    moltenbuffer.options,
                )
                if "output_type" in chunk:
                    restored.output_type = chunk["output_type"]
                extras = chunk.get("extras", {})
                if not isinstance(extras, dict):
                    raise IpynbIOError("Incorrect type for key 'extras': expected dict")
                restored.extras = extras
                output.chunks.append(restored)

        output.old = True
        output.status = OutputStatus.DONE

        moltenbuffer.outputs[span] = OutputBuffer(
            moltenbuffer.nvim,
            moltenbuffer.canvas,
            moltenbuffer.extmark_namespace,
            moltenbuffer.options,
        )
        moltenbuffer.outputs[span].output = output


def save(ipynb_kernel: IpynbKernel, nvim_buffer: int) -> Dict[str, Any]:
    """Save the current kernel state for the given buffer."""
    return {
        "version": 1,
        "kernel": ipynb_kernel.runtime.kernel_name,
        "content_checksum": ipynb_kernel._get_content_checksum(),
        "cells": [
            {
                "span": {
                    "begin": {
                        "lineno": span.begin.lineno,
                        "colno": span.begin.colno,
                    },
                    "end": {
                        "lineno": span.end.lineno,
                        "colno": span.end.colno,
                    },
                },
                "execution_count": output.output.execution_count,
                "status": output.output.status.value,
                "success": output.output.success,
                "chunks": [_serialize_chunk(chunk) for chunk in output.output.chunks],
            }
            for span, output in ipynb_kernel.outputs.items()
            if span.begin.bufno == nvim_buffer
        ],
    }
