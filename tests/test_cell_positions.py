import unittest
from types import SimpleNamespace

from ipynb_runtime.code_cell import CodeCell
from ipynb_runtime.position import DynamicPosition


class _Funcs:
    def __init__(self, positions):
        self.positions = positions
        self.queries = []
        self.next_id = 0

    def nvim_buf_set_extmark(self, _bufno, _namespace, lineno, colno, _opts):
        self.next_id += 1
        self.positions[self.next_id] = [lineno, colno]
        return self.next_id

    def nvim_buf_get_extmark_by_id(self, _bufno, _namespace, extmark_id, _opts):
        self.queries.append(extmark_id)
        return list(self.positions[extmark_id])

    def nvim_buf_del_extmark(self, _bufno, _namespace, _extmark_id):
        return True

    def nvim_buf_get_lines(self, _bufno, start, end, _strict):
        return self.lines[start:end]


class CellPositionTests(unittest.TestCase):
    def test_get_text_snapshots_each_dynamic_endpoint_once_and_tracks_moves(self):
        funcs = _Funcs({})
        funcs.lines = ["zero", "  first line", "last", "tail"]
        nvim = SimpleNamespace(funcs=funcs)
        begin = DynamicPosition(nvim, 1, 1, 1, 2)
        end = DynamicPosition(nvim, 1, 1, 2, 4)
        cell = CodeCell(nvim, begin, end)

        self.assertEqual(cell.get_text(nvim), "first line\nlast")
        self.assertEqual(funcs.queries, [begin.extmark_id, end.extmark_id])

        funcs.positions[begin.extmark_id] = [0, 1]
        funcs.positions[end.extmark_id] = [1, 2]
        funcs.queries.clear()
        self.assertEqual(cell.get_text(nvim), "ero\n  ")
        self.assertEqual(funcs.queries, [begin.extmark_id, end.extmark_id])


if __name__ == "__main__":
    unittest.main()
