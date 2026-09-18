import unittest
from unittest.mock import Mock

from ipynb_runtime.ipynb import compare_contents


class OutputMatchingTests(unittest.TestCase):
    def test_identical_source_needs_no_parser_rpc(self):
        nvim = Mock()
        cell = Mock()
        cell.get_text.return_value = 'print("hello")'
        self.assertTrue(compare_contents(nvim, {"source": 'print("hello")'}, cell, "python"))
        nvim.exec_lua.assert_not_called()
        nvim.lua._ipynb_remove_comments.assert_not_called()

    def test_different_source_uses_one_parser_rpc_and_preserves_result(self):
        for result in (True, False):
            nvim = Mock()
            nvim.exec_lua.return_value = result
            cell = Mock()
            cell.get_text.return_value = "print(1) # edited comment"
            self.assertEqual(compare_contents(nvim, {"source": "print(1)"}, cell, "python"), result)
            nvim.exec_lua.assert_called_once()
            self.assertEqual(nvim.exec_lua.call_args.args[1:],
                             ("print(1)\n", "print(1) # edited comment\n", "python"))


if __name__ == "__main__":
    unittest.main()
