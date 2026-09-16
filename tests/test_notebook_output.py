import unittest
from pathlib import Path
from tempfile import TemporaryDirectory
from types import SimpleNamespace
from unittest.mock import Mock, patch

import nbformat

from ipynb_runtime import Ipynb
from ipynb_runtime.ipynb import _notebook_output, handle_output_types, write_notebook_file
from ipynb_runtime.moltenbuffer import IpynbKernel
from ipynb_runtime.runtime import JupyterRuntime
from ipynb_runtime.runtime_state import RuntimeState
from ipynb_runtime.save_load import save, write_save_file
from ipynb_runtime.outputbuffer import render_control_chars
from ipynb_runtime.outputchunks import ErrorOutputChunk, Output, OutputStatus, TextOutputChunk
from ipynb_runtime.utils import IpynbException, notify_info


class TestNotebookOutput(unittest.TestCase):
    def test_render_control_chars_replays_terminal_updates(self):
        self.assertEqual(render_control_chars("abc\rXY"), "XYc")
        self.assertEqual(render_control_chars("abc\b\bXY"), "aXY")
        self.assertEqual(render_control_chars("abc\n\b\b\rXY"), "XYc")

    def test_done_cell_is_still_rendered_when_selection_changes(self):
        cell = object()
        buffer_ = SimpleNamespace(number=1)
        nvim = SimpleNamespace(
            current=SimpleNamespace(buffer=buffer_, window=SimpleNamespace(buffer=buffer_))
        )
        kernel = object.__new__(IpynbKernel)
        kernel.nvim = nvim
        kernel.buffers = [buffer_]
        output = SimpleNamespace(display_win=None)
        kernel.outputs = {cell: output}
        kernel.selected_cell = None
        kernel.output_statuses = {cell: OutputStatus.DONE}
        kernel.options = SimpleNamespace(virt_text_output=False)
        kernel.canvas = SimpleNamespace(present=Mock())
        kernel.updating_interface = False
        kernel.clear_empty_spans = Mock()
        kernel._get_selected_span = Mock(return_value=cell)
        kernel._show_selected = Mock()

        kernel.update_interface()

        kernel._show_selected.assert_called_once_with(cell)

        kernel._show_selected.reset_mock()
        kernel.should_show_floating_win = True
        kernel.update_interface()
        kernel._show_selected.assert_called_once_with(cell)

        kernel._show_selected.reset_mock()
        kernel.should_show_floating_win = False
        output.display_win = SimpleNamespace(valid=True)
        kernel.update_interface()
        kernel._show_selected.assert_called_once_with(cell)

    def test_show_output_uses_selected_imported_cell(self):
        cell = object()
        kernel = SimpleNamespace(
            current_output=None,
            selected_cell=cell,
            should_show_floating_win=False,
        )
        molten = object.__new__(Ipynb)
        molten._initialize_if_necessary = Mock()
        molten._get_current_buf_kernels = Mock(return_value=[kernel])
        molten._update_interface = Mock()

        Ipynb.command_show_output(molten)

        self.assertTrue(kernel.should_show_floating_win)
        molten._update_interface.assert_called_once_with()

    def test_shared_kernel_detaches_one_buffer_before_deinit(self):
        first_buffer = SimpleNamespace(number=1)
        second_buffer = SimpleNamespace(number=2)
        kernel = object.__new__(IpynbKernel)
        kernel.buffers = [first_buffer, second_buffer]
        kernel.kernel_id = "python3"
        kernel.clear_buffer = Mock()
        kernel.deinit = Mock()

        ipynb = object.__new__(Ipynb)
        ipynb.nvim = SimpleNamespace(current=SimpleNamespace(buffer=first_buffer))
        ipynb.buffers = {1: [kernel], 2: [kernel]}
        ipynb.ipynb_kernels = {"python3": kernel}

        ipynb._deinit_buffer([kernel], 1)

        self.assertEqual(ipynb.buffers, {2: [kernel]})
        self.assertEqual(kernel.buffers, [second_buffer])
        kernel.clear_buffer.assert_called_once_with(1)
        kernel.deinit.assert_not_called()
        self.assertIs(ipynb.ipynb_kernels["python3"], kernel)

        ipynb._deinit_buffer([kernel], 2)

        self.assertEqual(ipynb.buffers, {})
        self.assertEqual(kernel.clear_buffer.call_args_list, [((1,), {}), ((2,), {})])
        kernel.deinit.assert_called_once_with()
        self.assertEqual(ipynb.ipynb_kernels, {})

    def test_saved_chunks_keep_protocol_types_and_metadata_defaults(self):
        class Position:
            bufno = 1
            lineno = 0
            colno = 0

        class Span:
            begin = Position()
            end = Position()

        output = Output(4)
        stream = TextOutputChunk("stdout\n")
        stream.output_type = "stream"
        stream.extras = {"name": "stdout"}
        error = ErrorOutputChunk("ValueError", "bad", ["traceback line"])
        error.extras = {
            "ename": "ValueError",
            "evalue": "bad",
            "traceback": ["traceback line"],
        }
        output.status = OutputStatus.DONE
        output.chunks = [stream, error]
        kernel = SimpleNamespace(
            runtime=SimpleNamespace(kernel_name="python3"),
            _get_content_checksum=lambda: "checksum",
            outputs={Span(): SimpleNamespace(output=output)},
        )

        state = save(kernel, 1)

        self.assertEqual(len(state["cells"][0]["chunks"]), 2)
        self.assertEqual(state["cells"][0]["chunks"][0]["data"], {"text/plain": "stdout\n"})
        self.assertEqual(state["cells"][0]["chunks"][0]["metadata"], {})
        self.assertEqual(state["cells"][0]["chunks"][0]["output_type"], "stream")
        self.assertEqual(state["cells"][0]["chunks"][0]["extras"], {"name": "stdout"})
        self.assertEqual(state["cells"][0]["chunks"][1]["output_type"], "error")
        self.assertEqual(state["cells"][0]["chunks"][1]["extras"]["ename"], "ValueError")

    def test_import_export_roundtrip_keeps_stream_and_result_fields(self):
        kernel = SimpleNamespace(
            runtime=SimpleNamespace(_alloc_file=Mock()),
            options=SimpleNamespace(),
        )
        outputs = [
            {"output_type": "stream", "name": "stdout", "text": "out\n"},
            {"output_type": "stream", "name": "stderr", "text": "err\n"},
            {
                "output_type": "error",
                "ename": "ValueError",
                "evalue": "bad",
                "traceback": ["ValueError: bad"],
            },
            {
                "output_type": "execute_result",
                "execution_count": 3,
                "data": {"text/plain": "42"},
                "metadata": {},
            },
            {
                "output_type": "display_data",
                "data": {"text/plain": "shown"},
                "metadata": {},
                "transient": {"display_id": "wire-only"},
            },
        ]

        chunks = [handle_output_types(SimpleNamespace(), item["output_type"], kernel, item)[0] for item in outputs]
        roundtrip = [_notebook_output(nbformat, chunk, 3) for chunk in chunks]

        self.assertEqual([item["output_type"] for item in roundtrip], [item["output_type"] for item in outputs])
        self.assertEqual(roundtrip[0]["name"], "stdout")
        self.assertEqual(roundtrip[1]["name"], "stderr")
        self.assertEqual(roundtrip[0]["text"], "out\n")
        self.assertEqual(roundtrip[1]["text"], "err\n")
        self.assertEqual(roundtrip[2]["traceback"], ["ValueError: bad"])
        self.assertEqual(roundtrip[3]["execution_count"], 3)
        self.assertEqual(roundtrip[3]["data"], {"text/plain": "42"})
        self.assertEqual(roundtrip[4]["data"], {"text/plain": "shown"})
        self.assertNotIn("transient", roundtrip[4])
        nbformat.validate(
            nbformat.v4.new_notebook(
                cells=[nbformat.v4.new_code_cell("pass", execution_count=3, outputs=roundtrip)]
            )
        )

    def test_relative_atomic_writes_preserve_existing_files(self):
        with TemporaryDirectory() as temporary:
            existing = Path(temporary) / "state.json"
            existing.write_text("old", encoding="utf-8")
            with patch("ipynb_runtime.save_load.os.getcwd", return_value=temporary):
                write_save_file("state.json", {"value": 1})
            self.assertEqual(existing.read_text(encoding="utf-8"), '{"value": 1}')

            notebook = Path(temporary) / "notebook.ipynb"
            notebook.write_text("old notebook", encoding="utf-8")
            with patch("nbformat.write", side_effect=RuntimeError("interrupted")):
                with self.assertRaisesRegex(RuntimeError, "interrupted"):
                    write_notebook_file({}, str(notebook))
            self.assertEqual(notebook.read_text(encoding="utf-8"), "old notebook")

    def test_external_connection_starts_channels_and_uses_control_messages(self):
        with TemporaryDirectory() as temporary:
            connection_file = Path(temporary) / "kernel.json"
            connection_file.write_text('{"kernel_name": "python3"}', encoding="utf-8")
            client = Mock()
            manager = Mock()
            manager.client.return_value = client
            nvim = SimpleNamespace(exec_lua=Mock())
            options = SimpleNamespace(numpy_legacy_repr=False)

            with patch("ipynb_runtime.runtime.jupyter_client.manager.KernelManager", return_value=manager):
                runtime = JupyterRuntime(nvim, str(connection_file), "external", options)

            client.load_connection_file.assert_called_once_with(connection_file=str(connection_file))
            client.start_channels.assert_called_once_with()

            runtime.interrupt()
            message = client.session.msg.return_value
            client.session.msg.assert_called_once_with("interrupt_request", {})
            client.control_channel.send.assert_called_once_with(message)

            with self.assertRaisesRegex(IpynbException, "Cannot restart"):
                runtime.restart()
            client.shutdown.assert_not_called()
            runtime.deinit()
            client.stop_channels.assert_called_once_with()

    def test_input_polling_checks_readiness_method(self):
        runtime = object.__new__(JupyterRuntime)
        runtime.is_ready = Mock(return_value=False)
        runtime.kernel_client = Mock()

        runtime.tick_input()

        runtime.is_ready.assert_called_once_with()
        runtime.kernel_client.get_stdin_msg.assert_not_called()

    def test_execution_waits_for_kernel_ready_before_sending(self):
        runtime = object.__new__(JupyterRuntime)
        runtime.state = RuntimeState.STARTING
        runtime._pending_code = []
        client = SimpleNamespace()
        client.wait_for_ready = Mock()
        client.execute = Mock()
        runtime.kernel_client = client

        runtime.run_code("print('ready')")
        client.execute.assert_not_called()

        with patch(
            "ipynb_runtime.runtime.jupyter_client.blocking.client.BlockingKernelClient",
            type(client),
        ):
            runtime.tick(None)

        client.wait_for_ready.assert_called_once_with(timeout=0)
        client.execute.assert_called_once_with("print('ready')")

    def test_external_restart_does_not_clear_local_outputs(self):
        runtime = SimpleNamespace(
            external_kernel=True,
            restart=Mock(side_effect=IpynbException("Cannot restart an external kernel")),
        )
        outputs = {"cell": object()}
        kernel = object.__new__(IpynbKernel)
        kernel.runtime = runtime
        kernel.outputs = outputs

        with self.assertRaisesRegex(IpynbException, "Cannot restart"):
            kernel.restart(delete_outputs=True)

        self.assertIs(kernel.outputs, outputs)
        runtime.restart.assert_called_once_with()

    def test_notifications_pass_message_as_lua_arguments(self):
        nvim = SimpleNamespace(exec_lua=Mock())
        message = "kernel path ]] is still usable"

        notify_info(nvim, message)

        lua, *args = nvim.exec_lua.call_args.args
        self.assertNotIn(message, lua)
        self.assertIn("vim.log.levels[level]", lua)
        self.assertEqual(args, [message, "INFO"])


if __name__ == "__main__":
    unittest.main()
