import importlib.util
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory


ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("ipynb_setup", ROOT / "scripts" / "setup.py")
SETUP = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(SETUP)


class SetupTests(unittest.TestCase):
    def test_existing_non_environment_is_not_recreated(self):
        with TemporaryDirectory() as temporary:
            venv = Path(temporary) / ".venv"
            venv.mkdir()
            with self.assertRaisesRegex(RuntimeError, "refusing to recreate"):
                SETUP.install_runtime(venv, ROOT / "requirements.txt")


if __name__ == "__main__":
    unittest.main()
