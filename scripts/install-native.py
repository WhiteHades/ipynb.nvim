#!/usr/bin/env python3
"""Install ipynb.nvim for plain Neovim without a plugin manager."""

from __future__ import annotations

import os
from pathlib import Path
import subprocess
import sys


SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from setup import install_runtime


DEPENDENCIES = (
    ("jupytext.nvim", "https://github.com/goerz/jupytext.nvim.git", "v0.2.0"),
    ("quarto-nvim", "https://github.com/quarto-dev/quarto-nvim.git", None),
    ("otter.nvim", "https://github.com/jmbuhr/otter.nvim.git", None),
    ("nvim-treesitter", "https://github.com/nvim-treesitter/nvim-treesitter.git", None),
    ("image.nvim", "https://github.com/3rd/image.nvim.git", None),
)
PARSERS = ("markdown", "markdown_inline", "python")
NATIVE_LUA = (
    "local root=vim.env.IPYNB_NATIVE_ROOT; "
    "local python=vim.env.IPYNB_NATIVE_PYTHON; "
    "assert(root and root~='', 'IPYNB_NATIVE_ROOT is missing'); "
    "assert(python and python~='', 'IPYNB_NATIVE_PYTHON is missing'); "
    "vim.opt.rtp:prepend(root); vim.cmd('packloadall'); "
    "require('ipynb').setup({python=python, images=false}); "
    "local install=require('nvim-treesitter').install({'markdown','markdown_inline','python'}); "
    "assert(install and install.wait, 'nvim-treesitter install API is unavailable'); "
    "install:wait(300000); vim.cmd('runtime! plugin/rplugin.vim'); "
    "vim.cmd('UpdateRemotePlugins')"
)


def run(command: list[str], **kwargs):
    """Run a process without invoking a shell."""

    print("+", " ".join(command))
    return subprocess.run(command, check=True, **kwargs)


def nvim_data_path(nvim: str = "nvim") -> Path:
    """Ask a clean Neovim for its data directory."""

    result = run(
        [
            nvim,
            "-u",
            "NONE",
            "--headless",
            "-c",
            'lua io.stdout:write(vim.fn.stdpath("data"))',
            "-c",
            "qa!",
        ],
        capture_output=True,
        text=True,
    )
    value = result.stdout.strip()
    if not value:
        raise RuntimeError("Neovim returned an empty stdpath(data)")
    return Path(value).expanduser().resolve()


def native_plugin_path(data: Path) -> Path:
    return data / "site" / "pack" / "ipynb" / "start" / "ipynb.nvim"


def clone_dependencies(plugin_root: Path) -> None:
    """Clone missing sibling packages, preserving every existing path."""

    packages = plugin_root.parent
    packages.mkdir(parents=True, exist_ok=True)
    for name, url, ref in DEPENDENCIES:
        target = packages / name
        if target.exists():
            if not target.is_dir():
                raise RuntimeError(f"Dependency path exists and is not a directory: {target}")
            print(f"Reusing existing dependency: {target}")
            continue

        command = ["git", "clone", "--depth", "1"]
        if ref:
            command.extend(["--branch", ref])
        command.extend([url, str(target)])
        run(command)


def configure_neovim(plugin_root: Path, interpreter: Path, nvim: str = "nvim") -> None:
    """Load the native package, install parsers, and register remote plugins."""

    environment = os.environ.copy()
    environment.update(
        {
            "IPYNB_NATIVE_ROOT": str(plugin_root),
            "IPYNB_NATIVE_PYTHON": str(interpreter),
        }
    )
    run(
        [nvim, "-u", "NONE", "--headless", "-c", "lua local ok, err = pcall(function() " + NATIVE_LUA + "; end); "
         "if not ok then vim.api.nvim_err_writeln(tostring(err)); vim.cmd('cquit 1') end", "-c", "qa!"],
        env=environment,
        text=True,
    )


def install_native(nvim: str = "nvim") -> Path:
    """Install dependencies and the managed runtime for this native checkout."""

    plugin_root = Path(__file__).resolve().parents[1]
    data = nvim_data_path(nvim)
    expected_root = native_plugin_path(data)
    if plugin_root != expected_root:
        raise RuntimeError(
            "Run this script from the native checkout at "
            f"{expected_root}; current checkout is {plugin_root}"
        )

    clone_dependencies(plugin_root)
    interpreter = install_runtime(data / "ipynb.nvim" / "venv", plugin_root / "requirements.txt")
    configure_neovim(plugin_root, interpreter, nvim)
    print(f"Native ipynb.nvim installation is ready: {plugin_root}")
    return plugin_root


def main() -> int:
    try:
        install_native()
    except (OSError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f"ipynb.nvim native setup failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
