import json
import os
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest import TestCase
from unittest.mock import patch

import nbformat

from ipynb_runtime.notebook_io import NotebookIOError, read_notebook, write_notebook


def mtime(path: Path) -> dict[str, int]:
    value = path.stat().st_mtime_ns
    return {"sec": value // 1_000_000_000, "nsec": value % 1_000_000_000}


class NotebookIOTests(TestCase):
    def notebook(self) -> nbformat.NotebookNode:
        return nbformat.v4.new_notebook(
            metadata={"custom": {"kept": True}},
            cells=[
                nbformat.v4.new_code_cell(
                    "print(1)",
                    id="stable-cell",
                    metadata={"tag": "kept"},
                    execution_count=4,
                    outputs=[nbformat.v4.new_output("stream", name="stdout", text="1\n")],
                ),
                nbformat.v4.new_markdown_cell("![plot](attachment:plot.png)", id="stable-markdown"),
            ],
        )

    def write_fixture(self, directory: str) -> Path:
        path = Path(directory) / "notebook.ipynb"
        notebook = self.notebook()
        notebook.cells[1]["attachments"] = {"plot.png": {"image/png": "encoded-image"}}
        nbformat.write(notebook, path)
        return path

    def test_round_trip_preserves_metadata_ids_and_outputs(self):
        with TemporaryDirectory() as directory:
            path = self.write_fixture(directory)
            before = json.loads(path.read_text(encoding="utf-8"))
            result = read_notebook(str(path), "")

            stats = write_notebook(str(path), result["text"], mtime(path))

            after = json.loads(path.read_text(encoding="utf-8"))
            self.assertEqual(after["metadata"]["custom"], before["metadata"]["custom"])
            self.assertEqual(after["cells"][0]["id"], "stable-cell")
            self.assertEqual(after["cells"][0]["outputs"], before["cells"][0]["outputs"])
            self.assertEqual(after["cells"][1]["attachments"], before["cells"][1]["attachments"])
            self.assertEqual(stats["mtime"], mtime(path))

    def test_new_file_uses_template(self):
        with TemporaryDirectory() as directory:
            template = self.write_fixture(directory)
            target = Path(directory) / "new.ipynb"
            template_result = read_notebook("", str(template))
            target.write_text(template.read_text(encoding="utf-8"), encoding="utf-8")
            target.unlink()

            write_notebook(str(target), template_result["text"])

            self.assertTrue(target.exists())
            self.assertIn("plot.png", template_result["text"])

    def test_existing_file_mode_is_preserved(self):
        with TemporaryDirectory() as directory:
            path = self.write_fixture(directory)
            os.chmod(path, 0o640)
            result = read_notebook(str(path), "")

            write_notebook(str(path), result["text"], mtime(path))

            self.assertEqual(path.stat().st_mode & 0o777, 0o640)

    def test_changed_source_retains_matched_outputs_and_cell_metadata(self):
        with TemporaryDirectory() as directory:
            path = self.write_fixture(directory)
            result = read_notebook(str(path), "")
            changed = result["text"].replace("print(1)", "print(2)")

            write_notebook(str(path), changed, mtime(path))

            cell = json.loads(path.read_text(encoding="utf-8"))["cells"][0]
            self.assertEqual("".join(cell["source"]), "print(2)")
            self.assertEqual(cell["id"], "stable-cell")
            self.assertEqual(cell["metadata"], {"tag": "kept"})
            self.assertEqual(cell["execution_count"], 4)
            self.assertEqual("".join(cell["outputs"][0]["text"]), "1\n")

    def test_external_change_is_rejected_without_replacing_file(self):
        with TemporaryDirectory() as directory:
            path = self.write_fixture(directory)
            expected = mtime(path)
            path.write_text('{"changed": true}\n', encoding="utf-8")
            os.utime(path, ns=(path.stat().st_atime_ns, (expected["sec"] + 2) * 1_000_000_000))
            before = path.read_bytes()

            with self.assertRaisesRegex(NotebookIOError, "changed on disk"):
                write_notebook(str(path), "# changed", expected)

            self.assertEqual(path.read_bytes(), before)

    def test_conversion_failure_leaves_existing_file_untouched(self):
        with TemporaryDirectory() as directory:
            path = self.write_fixture(directory)
            before = path.read_bytes()
            with patch(
                "ipynb_runtime.notebook_io.write_notebook_file",
                side_effect=OSError("disk full"),
            ):
                with self.assertRaisesRegex(NotebookIOError, "disk full"):
                    write_notebook(str(path), "# changed", mtime(path))
            self.assertEqual(path.read_bytes(), before)

    def test_malformed_notebook_does_not_replace_file(self):
        with TemporaryDirectory() as directory:
            path = Path(directory) / "notebook.ipynb"
            path.write_text("not json", encoding="utf-8")
            before = path.read_bytes()

            with self.assertRaises(NotebookIOError):
                read_notebook(str(path), "")

            self.assertEqual(path.read_bytes(), before)

    def test_malformed_existing_notebook_does_not_replace_file_on_write(self):
        with TemporaryDirectory() as directory:
            path = Path(directory) / "notebook.ipynb"
            path.write_text("not json", encoding="utf-8")
            before = path.read_bytes()

            with self.assertRaises(NotebookIOError):
                write_notebook(str(path), "# changed")

            self.assertEqual(path.read_bytes(), before)


if __name__ == "__main__":
    import unittest

    unittest.main()
