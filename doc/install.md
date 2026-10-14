# Installation details

## Prerequisites

The lazy.nvim build and `:IpynbInstall` check `scripts/prerequisites.sh` first.
If required tools are ready, installation continues without changing them.
Otherwise, interactive Neovim opens a terminal with the install plan and asks
for confirmation. The terminal also accepts sudo prompts when needed.
After a successful check, installation continues with the Python helper,
Tree-sitter parsers, and Rust engine. Restart Neovim when installation finishes.

Headless Neovim stops when tools are missing and prints a shell command to run
in a terminal. It does not wait for confirmation or sudo input.

To install prerequisites before opening Neovim, download and run the standalone
helper with curl and Bash. No clone is needed.

```sh
ipynb_setup=$(curl -fsSL https://raw.githubusercontent.com/WhiteHades/ipynb.nvim/main/scripts/prerequisites.sh) &&
  bash -c "$ipynb_setup" -- --install
```

If you already have a checkout, `bash scripts/prerequisites.sh --install` works too.
The helper previews commands, asks before installing, and checks again afterwards.
Use `--check`, or omit `--install`, for a read-only check. `--yes` explicitly
accepts the install plan without a confirmation prompt, but sudo may still ask
for a password. `--no-images` skips ImageMagick, and `NO_COLOR=1` disables color.
Rust is the only execution backend. The old `--python-backend` flag and
`backend = "python"` option are no longer supported.

It supports apt, dnf, pacman, and Homebrew on macOS. System packages may need
sudo; pacman performs a full system upgrade. On macOS, finish Apple's
command-line tools installer before rerunning the helper if prompted.
Unsupported systems get a checklist. Only curl and Bash are needed to start
the helper; it can install Git along with the other missing prerequisites.

The Rust engine requires:

- Neovim 0.12+, uv 0.13.1+, and Rust/Cargo 1.88+.
- Git, curl, tar, pkg-config, a C/C++ compiler, and tree-sitter-cli 0.26.1+.
- ImageMagick's `magick` command and Kitty or Ghostty for inline plots.

The helper installs missing or outdated uv and Rust through the supported
system facility or an official upstream installer when packages are unavailable
or too old. The Rust fallback can select Rust 1.88.0 as your default toolchain.
The Rust engine and parsers build from source; no prebuilt engine is installed.

User installers do not change shell profiles. Ensure `~/.local/bin` and
`~/.cargo/bin` are on `PATH` before running uv or restarting Neovim:

```sh
export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"
```

A new private helper environment uses Python 3.15.0 through uv, so system Python,
`venv`, and `ensurepip` are not required. Installation runs the equivalent of
the following command from the plugin checkout:

```sh
uv run --no-project --python 3.15.0 -- scripts/setup.py \
  --venv ~/.local/share/nvim/ipynb.nvim/venv
```

Existing helper environments are reused. The helper environment stays separate
from notebook project environments.
The direct Python requirements are Jupytext and ipykernel. They supply the
Jupyter and nbformat libraries used for notebook conversion and kernel discovery.
The helper does not need pynvim, jiter, NumPy, Matplotlib, or Pillow. Install
plotting packages in each notebook project's environment when its cells need them.
The exact Python request avoids prereleases. uv 0.13.1 includes the final
Python 3.15.0 download; older uv catalogs can select a release candidate for 3.15.
Replace `~/.local/share/nvim` if you use a different Neovim data directory.

`[NEED]` after installation means a tool is still missing or too old. Check
`PATH` or install a newer upstream version. The checker exits with status 1
until required tools are ready.
It does not install a terminal or change your Neovim configuration.

## Custom Python and images

Set `opts.python` to use a custom helper environment, and install this
repository's `requirements.txt` into it first. `python3_host_prog` belongs to
other plugins and is neither read nor changed. See `:help ipynb-options`.
Set `opts = { images = false }` to disable plots, alongside `--no-images` above.
Image display uses ImageMagick and Lua providers, not Python plotting packages.

## Plain Neovim without a plugin manager

First, install the prerequisites with the standalone helper. Then, on Linux, run:

```sh
git clone https://github.com/WhiteHades/ipynb.nvim \
  ~/.local/share/nvim/site/pack/ipynb/start/ipynb.nvim
uv run --no-project --python 3.15.0 -- \
  ~/.local/share/nvim/site/pack/ipynb/start/ipynb.nvim/scripts/install-native.py
```

The native installer checks prerequisites before installing dependencies.
Add these lines to `~/.config/nvim/init.lua`, then restart:

```lua
require("image").setup({ processor = "magick_cli" })
require("ipynb").setup()
```

Without inline plots, pass `--no-images` to both the prerequisite helper and
`install-native.py`, then use only
`require("ipynb").setup({ images = false })` in your configuration.
If you changed your data directory, replace `~/.local/share/nvim` with the path
shown by `:lua print(vim.fn.stdpath("data"))`.

## Releases

To stay on a published version with lazy.nvim, add `version = "*"` to the
plugin specification. Omit it to follow the default branch. After updating,
run `:IpynbInstall` and restart Neovim to rebuild the Rust engine.
