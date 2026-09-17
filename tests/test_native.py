import importlib.util
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("ipynb_native", ROOT / "scripts" / "install-native.py")
NATIVE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(NATIVE)


class NativeInstallerTests(unittest.TestCase):
    def test_existing_dependency_directories_are_never_cloned(self):
        with TemporaryDirectory() as temporary:
            root = Path(temporary) / "ipynb.nvim"
            packages = root.parent
            root.mkdir()
            sentinels = {}
            for name, _, _ in NATIVE.DEPENDENCIES:
                target = packages / name
                target.mkdir()
                sentinel = target / "keep.txt"
                sentinel.write_text(name)
                sentinels[name] = sentinel

            with patch.object(NATIVE, "run") as run:
                NATIVE.clone_dependencies(root)

            run.assert_not_called()
            self.assertTrue(all(path.read_text() == path.parent.name for path in sentinels.values()))

if __name__ == "__main__":
    unittest.main()
