import importlib.util
import sys
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("ipynb_setup", ROOT / "scripts" / "setup.py")
SETUP = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(SETUP)


class SetupTests(unittest.TestCase):
    def test_requirements_cover_runtime_without_torch(self):
        requirements = {
            line.strip().lower()
            for line in (ROOT / "requirements.txt").read_text().splitlines()
            if line.strip() and not line.startswith("#")
        }
        self.assertTrue({"pynvim", "jupytext", "nbformat", "ipykernel", "jupyter_client", "numpy", "matplotlib", "pillow"} <= requirements)
        self.assertFalse(requirements & {"torch", "torchaudio", "torchvision"})

    def test_existing_environment_is_reused(self):
        with TemporaryDirectory() as temporary:
            venv = Path(temporary) / ".venv"
            interpreter = venv / "bin" / "python"
            interpreter.parent.mkdir(parents=True)
            interpreter.touch()
            commands = []

            with patch.object(SETUP, "run", side_effect=commands.append), patch.object(SETUP.shutil, "which", return_value=None):
                result = SETUP.install_runtime(venv, ROOT / "requirements.txt")

            self.assertEqual(result, interpreter.resolve())
            self.assertEqual(len(commands), 2)
            self.assertEqual(commands[0][:3], [str(interpreter.resolve()), "-m", "pip"])
            self.assertEqual(commands[1][:4], [str(interpreter.resolve()), "-m", "ipykernel", "install"])
            self.assertNotIn("venv", commands[0])

    def test_new_environment_uses_stdlib_when_uv_is_unavailable(self):
        with TemporaryDirectory() as temporary:
            venv = Path(temporary) / ".venv"
            commands = []

            def run(command):
                commands.append(command)
                if command[1:3] == ["-m", "venv"]:
                    interpreter = venv / "bin" / "python"
                    interpreter.parent.mkdir(parents=True)
                    interpreter.touch()

            with patch.object(SETUP, "run", side_effect=run), patch.object(SETUP.shutil, "which", return_value=None):
                SETUP.install_runtime(venv, ROOT / "requirements.txt")

            self.assertEqual(commands[0], [sys.executable, "-m", "venv", str(venv.resolve())])

    def test_existing_non_environment_is_not_recreated(self):
        with TemporaryDirectory() as temporary:
            venv = Path(temporary) / ".venv"
            venv.mkdir()
            with self.assertRaisesRegex(RuntimeError, "refusing to recreate"):
                SETUP.install_runtime(venv, ROOT / "requirements.txt")


if __name__ == "__main__":
    unittest.main()
