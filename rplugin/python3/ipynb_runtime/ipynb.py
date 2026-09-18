import os
import stat
import tempfile
from typing import Any, Dict

from pynvim.api import Buffer, Nvim
from ipynb_runtime.code_cell import CodeCell
from ipynb_runtime.moltenbuffer import IpynbKernel
from ipynb_runtime.outputbuffer import OutputBuffer
from ipynb_runtime.outputchunks import ErrorOutputChunk, Output, OutputStatus, to_outputchunk
from ipynb_runtime.position import DynamicPosition

from ipynb_runtime.utils import IpynbException, notify_error, notify_info, notify_warn

NOTEBOOK_VERSION = 4


def write_notebook_file(nb: Any, path: str) -> None:
    """Write a notebook atomically, including for relative paths."""
    import nbformat

    target = os.path.realpath(path)
    directory = os.path.dirname(os.path.abspath(target))
    try:
        mode = stat.S_IMODE(os.stat(target).st_mode)
    except FileNotFoundError:
        mode = None
    os.makedirs(directory, exist_ok=True)
    fd, temporary_path = tempfile.mkstemp(
        prefix=f".{os.path.basename(target)}.", suffix=".tmp", dir=directory
    )
    os.close(fd)
    try:
        nbformat.write(nb, temporary_path)
        with open(temporary_path, "rb") as file:
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


def get_default_import_export_file(nvim: Nvim, buffer: Buffer) -> str:
    # WARN: this is string containment checking, not array containment checking.
    if "nofile" in buffer.options["buftype"]:
        raise IpynbException("Buffer does not correspond to a file")

    file_name = nvim.funcs.expand("%")
    cwd = nvim.funcs.getcwd()
    full_path = os.path.join(cwd, file_name)
    return f"{os.path.splitext(full_path)[0]}.ipynb"


def import_outputs(nvim: Nvim, kernel: IpynbKernel, filepath: str):
    """Import outputs from an .ipynb file with the given name"""
    import nbformat

    if not filepath.endswith(".ipynb"):
        filepath += ".ipynb"

    if not os.path.exists(filepath):
        notify_warn(nvim, f"Cannot import from file: {filepath} because it does not exist.")
        return

    buf_line = 0
    buf = nvim.current.buffer
    buffer_contents = buf[:]
    nb = nbformat.read(filepath, as_version=NOTEBOOK_VERSION)

    ipynb_outputs: Dict[CodeCell, Output] = {}

    for cell in nb["cells"]:
        if cell["cell_type"] != "code" or "outputs" not in cell:
            continue

        nb_contents = cell["source"].split("\n")
        nb_line = 0
        while buf_line < len(buffer_contents):
            if len(nb_contents) == 0:
                break  # out of while loop
            if nb_contents[nb_line] != buffer_contents[buf_line]:
                # move on to the next buffer line, but reset the nb_line
                nb_line = 0
                buf_line += 1
                continue

            if nb_line >= len(nb_contents) - 1:
                # we're done. This is a match, we'll create the output
                output = Output(cell["execution_count"])
                output.old = True
                output.success = True
                if output.execution_count:
                    output.status = OutputStatus.DONE
                else:
                    output.status = OutputStatus.NEW

                for output_data in cell["outputs"]:
                    m_chunk, success = handle_output_types(nvim, output_data.get("output_type"), kernel, output_data)
                    output.chunks.append(m_chunk)
                    output.success &= success

                start = DynamicPosition(
                    nvim,
                    kernel.extmark_namespace,
                    buf.number,
                    buf_line - (len(nb_contents) - 1),
                    0,
                )
                end = DynamicPosition(
                    nvim, kernel.extmark_namespace, buf.number, buf_line, len(buf[buf_line])
                )
                code_cell = CodeCell(nvim, start, end)
                ipynb_outputs[code_cell] = output
                nb_line = 0
                buf_line += 1
                break  # out of the while loop

            buf_line += 1
            nb_line += 1

    failed = 0
    for span, output in ipynb_outputs.items():
        if kernel.try_delete_overlapping_cells(span):
            kernel.outputs[span] = OutputBuffer(
                kernel.nvim,
                kernel.canvas,
                kernel.extmark_namespace,
                kernel.options,
            )
            kernel.outputs[span].output = output
        else:
            failed += 1

    loaded = len(ipynb_outputs) - failed
    if loaded > 0:
        kernel.update_interface()

    if len(ipynb_outputs) == 0:
        notify_warn(nvim, "No cell outputs to import")
    elif loaded > 0:
        notify_info(nvim, f"Successfully loaded {loaded} outputs cells")
    if failed > 0:
        notify_error(
            nvim, f"Failed to load output for {failed} running cell that would be overridden"
        )

def handle_output_types(nvim: Nvim, output_type: str, kernel: IpynbKernel, output_data):
    chunk = None
    success = True
    match output_type:
        case "stream":
            chunk = to_outputchunk(
                nvim,
                kernel.runtime._alloc_file,
                {"text/plain": output_data.get("text", "")},
                output_data.get("metadata") or {},
                kernel.options,
            )
            chunk.output_type = "stream"
            chunk.extras = {"name": output_data.get("name", "stdout")}
        case "error":
            chunk = ErrorOutputChunk(
                output_data.get("ename", "Error"),
                output_data.get("evalue", ""),
                output_data.get("traceback", []),
            )
            chunk.extras = dict(output_data)
            success = False
        case _:
            chunk = to_outputchunk(
                nvim,
                kernel.runtime._alloc_file,
                output_data.get("data") or {},
                output_data.get("metadata") or {},
                kernel.options,
            )
            if output_type in ("display_data", "execute_result"):
                chunk.output_type = output_type
            extras = {}
            if output_type == "execute_result" and "execution_count" in output_data:
                extras["execution_count"] = output_data["execution_count"]
            if "transient" in output_data:
                extras["transient"] = output_data["transient"]
            chunk.extras = extras
    return chunk, success


def _notebook_output(nbformat, chunk, execution_count):
    """Build a valid nbformat output while retaining protocol-specific fields."""
    data = chunk.jupyter_data or {}
    extras = dict(chunk.extras or {})

    if chunk.output_type == "stream":
        return nbformat.v4.new_output(
            "stream",
            name=extras.get("name", "stdout"),
            text=data.get("text/plain", ""),
        )

    if chunk.output_type == "error":
        return nbformat.v4.new_output(
            "error",
            ename=extras.get("ename", "Error"),
            evalue=extras.get("evalue", ""),
            traceback=extras.get("traceback", []),
        )

    metadata = chunk.jupyter_metadata or {}
    if chunk.output_type == "execute_result":
        if extras.get("execution_count") is None:
            extras["execution_count"] = execution_count
        return nbformat.v4.new_output(
            "execute_result",
            data,
            metadata=metadata,
            execution_count=extras["execution_count"],
        )
    return nbformat.v4.new_output(
        chunk.output_type,
        data,
        metadata=metadata,
    )

def export_outputs(nvim: Nvim, kernel: IpynbKernel, filepath: str, overwrite: bool):
    """Export outputs of the current file/kernel to a .ipynb file with the given name."""
    import nbformat

    if not filepath.endswith(".ipynb"):
        filepath += ".ipynb"

    if not os.path.exists(filepath):
        notify_warn(nvim, f"Cannot export to file: {filepath} because it does not exist.")
        return

    nb = nbformat.read(filepath, as_version=NOTEBOOK_VERSION)

    ipynb_cells = sorted(kernel.outputs.items(), key=lambda x: x[0])

    if len(ipynb_cells) == 0:
        notify_warn(nvim, "No cell outputs to export")
        return

    nb_cells = list(filter(lambda x: x["cell_type"] == "code", nb["cells"]))
    nb_index = 0
    lang = kernel.runtime.kernel_manager.kernel_spec.language  # type: ignore
    for mcell in ipynb_cells:
        matched = False
        while nb_index < len(nb_cells):
            code_cell, output = mcell
            nb_cell = nb_cells[nb_index]
            nb_index += 1

            if compare_contents(nvim, nb_cell, code_cell, lang):
                matched = True
                outputs = [
                    _notebook_output(nbformat, chunk, output.output.execution_count)
                    for chunk in output.output.chunks
                ]
                nb_cell["outputs"] = outputs
                nb_cell["execution_count"] = output.output.execution_count
                break  # break out of the while loop

        if not matched:
            notify_error(
                nvim,
                f"No cell matching cell at line: {mcell[0].begin.lineno + 1} in notebook: {filepath}. Bailing.",
            )
            return

    if overwrite:
        write_to = filepath
    else:
        head, tail = os.path.split(filepath)
        write_to = os.path.join(head or ".", f"copy-of-{tail}")

    notify_info(nvim, f"Exporting {len(ipynb_cells)} cell output(s) to {write_to}")
    write_notebook_file(nb, write_to)


def compare_contents(nvim: Nvim, nb_cell, code_cell: CodeCell, lang: str) -> bool:
    ipynb_contents = code_cell.get_text(nvim)
    if nb_cell["source"] == ipynb_contents:
        return True
    return nvim.exec_lua(
        "local left, right, lang = ...; "
        "local clean = require('ipynb.molten_remove_comments').remove_comments; "
        "return clean(left, lang) == clean(right, lang)",
        nb_cell["source"] + "\n", ipynb_contents + "\n", lang,
    )
