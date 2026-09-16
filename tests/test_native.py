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
    def test_dependencies_are_unmodified_and_render_markdown_is_not_installed(self):
        names = {name for name, _, _ in NATIVE.DEPENDENCIES}
        self.assertEqual(names, {"jupytext.nvim", "quarto-nvim", "otter.nvim", "nvim-treesitter", "image.nvim"})
        self.assertEqual(dict((name, ref) for name, _, ref in NATIVE.DEPENDENCIES)["jupytext.nvim"], "v0.2.0")

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

    def test_missing_dependency_uses_shallow_git_clone_and_version_ref(self):
        with TemporaryDirectory() as temporary:
            root = Path(temporary) / "ipynb.nvim"
            root.mkdir()
            commands = []

            with patch.object(NATIVE, "run", side_effect=commands.append):
                NATIVE.clone_dependencies(root)

            self.assertEqual(len(commands), len(NATIVE.DEPENDENCIES))
            first = commands[0]
            self.assertEqual(first[:4], ["git", "clone", "--depth", "1"])
            self.assertIn("--branch", first)
            self.assertIn("v0.2.0", first)
            self.assertNotIn("--recurse-submodules", first)

    def test_clean_neovim_data_lookup_uses_argument_list(self):
        result = type("Result", (), {"stdout": "/tmp/nvim-data\n"})()
        with patch.object(NATIVE, "run", return_value=result) as run:
            data = NATIVE.nvim_data_path("nvim-custom")

        self.assertEqual(data, Path("/tmp/nvim-data").resolve())
        command = run.call_args.args[0]
        self.assertEqual(command[:4], ["nvim-custom", "-u", "NONE", "--headless"])
        self.assertNotIn("shell", run.call_args.kwargs)
        self.assertFalse(run.call_args.kwargs.get("shell", False))

    def test_native_install_uses_data_paths_without_touching_cwd(self):
        data = Path("/tmp/native-data").resolve()
        expected = NATIVE.native_plugin_path(data)
        with patch.object(NATIVE, "nvim_data_path", return_value=data), \
             patch.object(NATIVE, "clone_dependencies"), \
             patch.object(NATIVE, "install_runtime", return_value=data / "ipynb.nvim/venv/bin/python") as runtime, \
             patch.object(NATIVE, "configure_neovim") as configure:
            with patch.dict(NATIVE.__dict__, {"__file__": str(expected / "scripts/install-native.py")}):
                result = NATIVE.install_native("nvim-custom")

        self.assertEqual(result, expected)
        runtime.assert_called_once_with(data / "ipynb.nvim/venv", expected / "requirements.txt")
        configure.assert_called_once()


if __name__ == "__main__":
    unittest.main()
