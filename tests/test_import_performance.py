from types import SimpleNamespace
from unittest import TestCase
from unittest.mock import patch

from ipynb_runtime.ipynb import import_outputs


class _Buffer:
    number = 1

    def __init__(self, lines):
        self.lines = lines

    def __getitem__(self, item):
        return self.lines[item]


class _Span:
    def __init__(self, index):
        self.index = index


class _OutputBuffer:
    def __init__(self, *_args):
        self.output = None


class _Kernel:
    def __init__(self, nvim):
        self.nvim = nvim
        self.extmark_namespace = 1
        self.canvas = object()
        self.options = object()
        self.outputs = {}
        self.update_calls = 0
        self.update_sizes = []

    def try_delete_overlapping_cells(self, _span):
        return True

    def update_interface(self):
        self.update_calls += 1
        self.update_sizes.append(len(self.outputs))


class ImportPerformanceTests(TestCase):
    def test_many_cell_import_updates_interface_once_and_keeps_outputs(self):
        count = 40
        lines = [line for index in range(count) for line in (f"cell_{index}()", "")]
        cells = [
            {
                "cell_type": "code",
                "source": f"cell_{index}()",
                "execution_count": index + 1,
                "outputs": [{"output_type": "stream", "text": f"output {index}"}],
            }
            for index in range(count)
        ]
        nvim = SimpleNamespace(current=SimpleNamespace(buffer=_Buffer(lines)))
        kernel = _Kernel(nvim)
        spans = iter(range(count))

        with patch("ipynb_runtime.ipynb.os.path.exists", return_value=True), patch(
            "nbformat.read", return_value={"cells": cells}
        ), patch(
            "ipynb_runtime.ipynb.CodeCell", side_effect=lambda *_args: _Span(next(spans))
        ), patch("ipynb_runtime.ipynb.DynamicPosition", return_value=object()), patch(
            "ipynb_runtime.ipynb.OutputBuffer", _OutputBuffer
        ), patch(
            "ipynb_runtime.ipynb.handle_output_types", side_effect=lambda *_args: ("chunk", True)
        ), patch("ipynb_runtime.ipynb.notify_info"), patch("ipynb_runtime.ipynb.notify_warn"), patch(
            "ipynb_runtime.ipynb.notify_error"
        ):
            import_outputs(nvim, kernel, "saved.ipynb")

        self.assertEqual(kernel.update_calls, 1)
        self.assertEqual(kernel.update_sizes, [count])
        self.assertEqual(len(kernel.outputs), count)
        self.assertEqual([output.output.execution_count for output in kernel.outputs.values()], list(range(1, count + 1)))


if __name__ == "__main__":
    import unittest

    unittest.main()
