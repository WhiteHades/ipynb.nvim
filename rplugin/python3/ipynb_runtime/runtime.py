from datetime import datetime
from typing import Optional, Tuple, List, Dict, Generator, IO, Any
from contextlib import contextmanager
from queue import Empty as EmptyQueueException
import os
import tempfile
import json

import jupyter_client
from pynvim import Nvim

from ipynb_runtime.options import IpynbOptions
from ipynb_runtime.outputchunks import (
    Output,
    MimetypesOutputChunk,
    ErrorOutputChunk,
    TextOutputChunk,
    OutputStatus,
    to_outputchunk,
    clean_up_text,
)
from ipynb_runtime.runtime_state import RuntimeState
from ipynb_runtime.display import NUMPY_STARTUP
from ipynb_runtime.jupyter_server_api import JupyterAPIClient, JupyterAPIManager
from ipynb_runtime.utils import IpynbException


class JupyterRuntime:
    state: RuntimeState
    kernel_name: str
    kernel_id: str

    kernel_manager: jupyter_client.KernelManager | JupyterAPIManager  # type: ignore
    kernel_client: jupyter_client.KernelClient | JupyterAPIClient  # type: ignore

    allocated_files: List[str]

    options: IpynbOptions
    nvim: Nvim

    def __init__(self, nvim: Nvim, kernel_name: str, kernel_id: str, options: IpynbOptions):
        self.state = RuntimeState.STARTING
        self.kernel_name = kernel_name
        self.kernel_id = kernel_id
        self.nvim = nvim
        self._pending_code: List[str] = []
        self.nvim.exec_lua("_ipynb_prompt_stdin = require('ipynb.molten_prompt').prompt_stdin")

        if kernel_name.startswith("http://") or kernel_name.startswith("https://"):
            self.external_kernel = False
            self.kernel_manager = JupyterAPIManager(kernel_name)
            self.kernel_manager.start_kernel()
            self.kernel_client = self.kernel_manager.client()
            self.kernel_client.start_channels()
            self.options = options
        elif ".json" not in self.kernel_name:
            self.external_kernel = False
            self.kernel_manager = jupyter_client.manager.KernelManager(kernel_name=kernel_name)
            startup = []
            if options.numpy_legacy_repr and self.kernel_manager.kernel_spec.language == "python":
                startup = ["--IPKernelApp.exec_lines=" + json.dumps([NUMPY_STARTUP])]
            self.kernel_manager.start_kernel(extra_arguments=startup)
            self.kernel_client = self.kernel_manager.client()
            assert isinstance(
                self.kernel_client,
                jupyter_client.blocking.client.BlockingKernelClient,
            )
            self.kernel_client.start_channels()
            self.kernel_client.connection_file = (
                f"{self.kernel_client.data_dir}/runtime/kernel-{self.kernel_manager.kernel_id}.json"
            )
            os.makedirs(os.path.dirname(self.kernel_client.connection_file), mode=0o700, exist_ok=True)
            self.kernel_client.write_connection_file()
        else:
            kernel_file = kernel_name
            self.external_kernel = True
            # Opening JSON file
            try:
                with open(kernel_file) as file:
                    kernel_json = json.load(file)
            except FileNotFoundError:
                raise ValueError(f"Could not find kernel file at path: {kernel_file}")

            # we have a kernel json
            self.kernel_manager = jupyter_client.manager.KernelManager(
                kernel_name=kernel_json["kernel_name"]
            )
            self.kernel_client = self.kernel_manager.client()
            self.kernel_client.load_connection_file(connection_file=kernel_file)
            self.kernel_client.start_channels()

        self.allocated_files = []
        self.options = options

    def is_ready(self) -> bool:
        return self.state.value > RuntimeState.STARTING.value

    def deinit(self) -> None:
        for path in self.allocated_files:
            if os.path.exists(path):
                os.remove(path)

        if self.external_kernel is False:
            self.kernel_client.cleanup_connection_file()
            self.kernel_client.shutdown()
        else:
            self.kernel_client.stop_channels()

    def interrupt(self) -> None:
        if self.external_kernel:
            message = self.kernel_client.session.msg("interrupt_request", {})
            self.kernel_client.control_channel.send(message)
        else:
            self.kernel_manager.interrupt_kernel()

    def restart(self) -> None:
        if self.external_kernel:
            raise IpynbException("Cannot restart a kernel opened from an external connection file")
        self._pending_code.clear()
        self.state = RuntimeState.STARTING
        self.kernel_manager.restart_kernel()

    def run_code(self, code: str) -> None:
        # KernelClient.wait_for_ready() flushes IOPub. Queue requests until the
        # readiness check has completed so it cannot discard their output.
        if not self.is_ready():
            self._pending_code.append(code)
            return
        self.kernel_client.execute(code)

    def _run_pending_code(self) -> None:
        pending_code, self._pending_code = self._pending_code, []
        for code in pending_code:
            self.kernel_client.execute(code)

    @contextmanager
    def _alloc_file(
        self, extension: str, mode: str
    ) -> Generator[Tuple[str, IO[bytes]], None, None]:
        with tempfile.NamedTemporaryFile(suffix="." + extension, mode=mode, delete=False) as file:
            path = file.name
            yield path, file
        self.allocated_files.append(path)

    def _append_chunk(
        self,
        output: Output,
        data: Dict[str, Any],
        metadata: Dict[str, Any],
        output_type: str = "display_data",
        extras: Optional[Dict[str, Any]] = None,
    ) -> None:
        if self.options.show_mimetype_debug:
            output.chunks.append(MimetypesOutputChunk(list(data.keys())))

        if output.success:
            chunk = to_outputchunk(self.nvim, self._alloc_file, data, metadata, self.options)
            chunk.output_type = output_type
            chunk.extras = dict(extras or {})
            output.chunks.append(chunk)
            if isinstance(chunk, TextOutputChunk) and chunk.text.startswith("\r"):
                output.merge_text_chunks()

    def _tick_one(self, output: Output, message_type: str, content: Dict[str, Any]) -> bool:
        def copy_on_demand(content_ctor):
            if self.options.copy_output:
                import pyperclip

                if type(content_ctor) is str:
                    pyperclip.copy(content_ctor)
                else:
                    pyperclip.copy(content_ctor())

        if output._should_clear:
            output.chunks.clear()
            output._should_clear = False

        if message_type == "execute_input":
            output.execution_count = content["execution_count"]
            if self.external_kernel is False:
                if output.status == OutputStatus.DONE:
                    return False
                if output.status == OutputStatus.HOLD:
                    output.status = OutputStatus.RUNNING
                    output.start_time = datetime.now()
                elif output.status == OutputStatus.RUNNING:
                    output.status = OutputStatus.DONE
                else:
                    raise ValueError("bad value for output.status: %r" % output.status)
            return True
        elif message_type == "status":
            execution_state = content["execution_state"]
            assert execution_state != "starting"
            if execution_state == "idle":
                self.state = RuntimeState.IDLE
                output.status = OutputStatus.DONE
                return True
            elif execution_state == "busy":
                self.state = RuntimeState.RUNNING
                return True
            else:
                return False
        elif message_type == "execute_reply":
            # This doesn't really give us any relevant information.
            return False
        elif message_type == "execute_result":
            self._append_chunk(
                output,
                content["data"],
                content["metadata"],
                output_type="execute_result",
                extras={"execution_count": content.get("execution_count", output.execution_count)},
            )
            if "text/plain" in content["data"]:
                copy_on_demand(content["data"]["text/plain"])
            return True
        elif message_type == "error":
            output.success = False
            chunk = ErrorOutputChunk(content["ename"], content["evalue"], content["traceback"])
            chunk.extras = content
            output.chunks.append(chunk)

            copy_on_demand(lambda: "\n\n".join(map(clean_up_text, content["traceback"])))
            return True
        elif message_type == "stream":
            copy_on_demand(content["text"])
            self._append_chunk(
                output,
                {"text/plain": content["text"]},
                {},
                output_type="stream",
                extras={"name": content.get("name", "stdout")},
            )
            return True
        elif message_type == "display_data":
            self._append_chunk(
                output,
                content["data"],
                content["metadata"],
                output_type="display_data",
                extras={"transient": content["transient"]} if "transient" in content else {},
            )
            return True
        elif message_type == "update_display_data":
            # We don't really want to bother with this type of message.
            return False
        elif message_type == "clear_output":
            if content["wait"]:
                output._should_clear = True
            else:
                output.chunks.clear()
            return True
        # TODO: message_type == 'debug'?
        else:
            return False

    def tick(self, output: Optional[Output]) -> bool:
        did_stuff = False

        assert isinstance(
            self.kernel_client,
            (
                jupyter_client.blocking.client.BlockingKernelClient,
                JupyterAPIClient,
            ),
        )

        if not self.is_ready():
            try:
                self.kernel_client.wait_for_ready(timeout=0)
                self.state = RuntimeState.IDLE
                did_stuff = True
                self._run_pending_code()
            except RuntimeError:
                return False

        if output is None:
            return did_stuff

        while True:
            try:
                message = self.kernel_client.get_iopub_msg(timeout=0)

                if "content" not in message or "msg_type" not in message:
                    continue

                did_stuff_now = self._tick_one(output, message["msg_type"], message["content"])
                did_stuff = did_stuff or did_stuff_now

                if output.status == OutputStatus.DONE:
                    break
            except EmptyQueueException:
                break

        return did_stuff

    def tick_input(self):
        """Tick to check input_requests"""
        if not self.is_ready():
            return

        assert isinstance(
            self.kernel_client,
            (jupyter_client.blocking.client.BlockingKernelClient,
             JupyterAPIClient),
        )

        try:
            msg = self.kernel_client.get_stdin_msg(timeout=0)
            if msg is not None:
                self.take_input(msg)
        except EmptyQueueException:
            pass

    def take_input(self, msg):
        if msg["msg_type"] == "input_request":
            self.nvim.lua._ipynb_prompt_stdin(self.kernel_id, msg["content"]["prompt"])


def get_available_kernels() -> List[str]:
    return list(jupyter_client.kernelspec.find_kernel_specs().keys())  # type: ignore
