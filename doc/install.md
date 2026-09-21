# Installation details

## Prerequisites

Download and run the standalone helper with curl and Bash. No clone is needed.

```sh
ipynb_setup=$(curl -fsSL https://raw.githubusercontent.com/WhiteHades/ipynb.nvim/main/scripts/prerequisites.sh) &&
  bash -c "$ipynb_setup" -- --install
```

If you already have a checkout, `bash scripts/prerequisites.sh --install` works too.
The helper previews commands, asks before installing, and checks again afterwards.
Run it without `--install` for a read-only check. `--yes` accepts the install
plan, `--no-images` skips ImageMagick, and `NO_COLOR=1` disables color.

It supports apt, dnf, pacman, and Homebrew on macOS. System packages may need
sudo; pacman performs a full system upgrade. On macOS, finish Apple's
command-line tools installer before rerunning the helper if prompted.
Unsupported systems get a checklist. Only curl and Bash are needed to start
the helper; it can install Git along with the other missing prerequisites.

Required tools:

- Neovim 0.12+, Python 3.10+, Rust/Cargo 1.85+.
- Git, curl, tar, a C/C++ compiler, and tree-sitter-cli 0.26.1+.
- ImageMagick's `magick` command and Kitty or Ghostty for inline plots.
- Optional `uv`; otherwise Python needs `venv` and `ensurepip`.

`[NEED]` after installation means a tool is still missing or too old. Check
`PATH` or install a newer upstream version. Cargo tools usually live in
`~/.cargo/bin`. The checker exits with status 1 until required tools are ready.
It does not install a terminal or change your Neovim configuration.

## Custom Python and images

If you set `python3_host_prog`, install this repository's `requirements.txt`
into that environment first. See `:help ipynb-options` for custom runtimes.
Set `opts = { images = false }` to disable plots, alongside `--no-images` above.

## Plain Neovim without a plugin manager

on linux, run:

```sh
git clone https://github.com/WhiteHades/ipynb.nvim \
  ~/.local/share/nvim/site/pack/ipynb/start/ipynb.nvim
python3 ~/.local/share/nvim/site/pack/ipynb/start/ipynb.nvim/scripts/install-native.py
```

add these lines to `~/.config/nvim/init.lua`, then restart:

```lua
require("image").setup({ processor = "magick_cli" })
require("ipynb").setup()
```

without inline plots, use only `require("ipynb").setup({ images = false })`.
if you changed your data directory, replace `~/.local/share/nvim` with the path
shown by `:lua print(vim.fn.stdpath("data"))`.

## Releases

To stay on a published version with lazy.nvim, add `version = "*"` to the
plugin specification. Omit it to follow the default branch. After updating,
run `:IpynbInstall` and restart Neovim to rebuild the Rust engine.
