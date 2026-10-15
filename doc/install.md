# Installation

## Prerequisites

Start with the [README](../README.md#install). Installation asks before adding
missing tools. To install them separately, run:

```sh
ipynb_setup=$(curl -fsSL https://raw.githubusercontent.com/WhiteHades/ipynb.nvim/main/scripts/prerequisites.sh) &&
  bash -c "$ipynb_setup" -- --install
```

From a checkout, use `bash scripts/prerequisites.sh --install`.
Use `--check` to check tools, `--yes` to accept installation, or `--no-images`
to skip ImageMagick. Set `NO_COLOR=1` to disable color.

Supported package managers are apt, dnf, pacman, and Homebrew. Installation may
need sudo. Pacman performs a full system upgrade. The Rust installer may set
Rust 1.88.0 as your default toolchain. On macOS, finish Apple's command line
tools installer if prompted, then retry.

Requirements:

1. Neovim 0.12+, uv 0.13.1+, and Rust/Cargo 1.88+.
2. Git, curl, tar, `pkg-config`, a C/C++ compiler, and `tree-sitter-cli` 0.26.1+.
3. ImageMagick's `magick` command and Kitty or Ghostty for inline plots.

Add the user tool directories to `PATH`:

```sh
export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"
```

## Custom Python and images

Set `opts.python` to an existing helper's Python after installing
`requirements.txt` there. Install plotting packages in each notebook project.
See `:help ipynb-options` for project environments and kernel selection.

To disable images, set `opts = { images = false }` and pass `--no-images`
to the prerequisite installer.

In tmux, enable `allow-passthrough on` and `focus-events on`. Reattach existing
clients after changing focus events. Notebook plots hide when you leave their
pane and return when you come back. Top status rows and split offsets are included.

## Plain Neovim without a plugin manager

First, install the prerequisites with the standalone helper. Then, on Linux, run:

```sh
git clone https://github.com/WhiteHades/ipynb.nvim \
  ~/.local/share/nvim/site/pack/ipynb/start/ipynb.nvim
uv run --no-project --python 3.15.0 -- \
  ~/.local/share/nvim/site/pack/ipynb/start/ipynb.nvim/scripts/install-native.py
```

Add to `~/.config/nvim/init.lua`, then restart Neovim:

```lua
require("image").setup({
  processor = "magick_cli",
  editor_only_render_when_focused = false,
  tmux_show_only_in_active_window = true,
})
require("ipynb").setup()
```

To disable images, also pass `--no-images` to `install-native.py` and use only
`require("ipynb").setup({ images = false })`.

For a custom data directory, replace `~/.local/share/nvim` with the path from
`:lua print(vim.fn.stdpath("data"))`.

## Installation problems

Run `:checkhealth ipynb`. If the installer reports `[NEED]`, check `PATH` or
update the named tool. For headless installation, run the printed setup command
in a terminal first.

## Releases

Add `version = "*"` to use published releases, or omit it to follow the default
branch. After updates, run `:IpynbInstall` and restart Neovim. For a custom
helper, update its `requirements.txt` packages first.
