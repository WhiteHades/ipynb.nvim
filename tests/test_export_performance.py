from pathlib import Path
from tempfile import TemporaryDirectory
from types import SimpleNamespace
from unittest import TestCase
from unittest.mock import Mock, patch

import nbformat

from ipynb_runtime.ipynb import export_outputs
from ipynb_runtime.outputchunks import Output, TextOutputChunk


class ExportPerformanceTests(TestCase):
    def test_unchanged_outputs_skip_write_but_changed_outputs_and_copies_are_saved(self):
        with TemporaryDirectory() as directory:
            path = Path(directory) / "saved.ipynb"
            nbformat.write(nbformat.v4.new_notebook(cells=[nbformat.v4.new_code_cell(
                "print(1)", execution_count=1,
                outputs=[nbformat.v4.new_output("stream", name="stdout", text="1\n")],
            )]), path)
            span = Mock()
            span.begin._get_pos.return_value = [0, 0]
            span.get_text.return_value = "print(1)"
            output = Output(1)
            chunk = TextOutputChunk("1\n")
            chunk.output_type = "stream"
            chunk.jupyter_data = {"text/plain": "1\n"}
            chunk.extras = {"name": "stdout"}
            output.chunks = [chunk]
            kernel = SimpleNamespace(outputs={span: SimpleNamespace(output=output)},
                                     runtime=SimpleNamespace(kernel_manager=SimpleNamespace(
                                         kernel_spec=SimpleNamespace(language="python"))))
            with patch("ipynb_runtime.ipynb.write_notebook_file") as write, patch(
                "ipynb_runtime.ipynb.notify_info"
            ):
                export_outputs(Mock(), kernel, str(path), True)
                write.assert_not_called()
                span.begin._get_pos.assert_called_once()

                export_outputs(Mock(), kernel, str(path), False)
                self.assertEqual(write.call_args.args[1], str(path.with_name("copy-of-saved.ipynb")))
                write.reset_mock()

                chunk.jupyter_data = {"text/plain": "updated\n"}
                export_outputs(Mock(), kernel, str(path), True)
                self.assertEqual(write.call_args.args[0].cells[0].outputs[0].text, "updated\n")
                write.reset_mock()

                chunk.jupyter_data = {"text/plain": "1\n"}
                output.execution_count = 2
                export_outputs(Mock(), kernel, str(path), True)
                self.assertEqual(write.call_args.args[0].cells[0].execution_count, 2)
