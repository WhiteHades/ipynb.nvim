# ipynb.nvim

jupyter notebooks in neovim. edit cells, run code, view plots, and reopen saved
outputs. works with lazyvim, lazy.nvim, or neovim's built-in package loader.

[![a notebook running in neovim](assets/notebook.png)](assets/notebook.png)

## install

### check and install prerequisites

run the setup helper from a checkout of this repo:

```sh
git clone https://github.com/WhiteHades/ipynb.nvim
cd ipynb.nvim
bash scripts/prerequisites.sh --install
```

it shows what's ready or missing, previews the install commands, asks before
running them, and checks again afterwards. then add the plugin below; notebook
packages, parsers, and the Rust engine install automatically.

already have the repo? run the script there. for lazy.nvim, it is usually at
`~/.local/share/nvim/lazy/ipynb.nvim/scripts/prerequisites.sh`.
if git is missing, download and extract the repo's zip from GitHub first.

```sh
bash scripts/prerequisites.sh              # check only, without changing anything
bash scripts/prerequisites.sh --install --no-images  # skip inline plot prerequisites
bash scripts/prerequisites.sh --install --yes        # accept the printed install plan
```

<details>
<summary>requirements and supported systems</summary>

requires Bash. supports apt, dnf, pacman, and Homebrew on macOS. package
installation may ask for sudo; pacman also performs a full system upgrade.
tree-sitter-cli installs through Cargo. macOS may also need Apple's command-line
tools installer to finish before rerunning the script. unsupported systems get
a checklist.

required: neovim 0.12+, python 3.10+, Rust/Cargo 1.85+, git, curl, tar, a C/C++
compiler and tree-sitter-cli 0.26.1+. inline plots need ImageMagick's `magick`
command and a Kitty-compatible terminal such as Kitty or Ghostty. install the
terminal yourself. `uv` is optional; without it, Python needs `venv` and
`ensurepip`. to disable plots, also set `opts = { images = false }` below.

`[OK]` means ready, `[NEED]` needs fixing, and `[INFO]` is advisory. color is
optional; `NO_COLOR=1` disables it. checks exit with status 1 if a required
tool is missing or too old. distribution packages can lag behind: install a
newer upstream version or fix `PATH` if `[NEED]` remains after installation.
the script leaves your Neovim configuration alone.

</details>

### lazyvim or lazy.nvim

1. create `~/.config/nvim/lua/plugins/ipynb.lua` with:

   ```lua
   return {
     { "WhiteHades/ipynb.nvim", lazy = false, opts = {} },
   }
   ```

2. run `:Lazy sync`, then restart neovim. notebook tools install automatically.
3. open a notebook with `nvim example.ipynb`. a new path creates a blank notebook.

for plain lazy.nvim, add the same plugin specification to your existing setup.
if you already set `python3_host_prog`, install this repo's `requirements.txt`
into that environment first; `:help ipynb-options` explains custom runtimes.

<details>
<summary>plain neovim without a plugin manager</summary>

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

</details>

## use

put the cursor inside a cell. `:Ipynb` opens the action menu, so you do not need
to memorise the shortcuts. keys below use normal mode and the default
`<localleader>` of `\`.

| key | action |
| --- | --- |
| `\r` or ctrl-enter | run cell; `\r` also runs a visual selection |
| shift-enter | run and move to the next code cell |
| `\R` / `\a` | run all / run this cell and above |
| `\b` / `\m` | insert a code / markdown cell below |
| `\o` | open full output; scroll with normal vim motions |
| `\O` | close output; `q` or escape also work inside its window |
| `\s` / `\i` | interrupt / restart the kernel |
| `\n` | open the action menu |
| `\x` | enlarge the current cell's first image in the terminal |

### output and plots

press `\o` to open and focus the full output window. scroll with normal Vim
motions, then press `q`, escape, or `\O` to close it. `\O` also closes output
from the notebook.

to enlarge a plot, enable the mouse with `:set mouse=a` and click the image in
normal or insert mode. it opens in a centered terminal viewer and resizes with
your terminal. press `q`, escape, or `\O` to close it. existing mouse mappings
take precedence. without the mouse, put the cursor in the cell and use `\x` or
`:IpynbImagePopup` to open its first image. no desktop image app is needed.

| command | action |
| --- | --- |
| `:Ipynb` | choose a notebook action |
| `:IpynbEnterOutput` / `:IpynbHideOutput` | open and focus / close full output |
| `:IpynbImagePopup` | enlarge the current cell's first image |
| `:IpynbOpenInBrowser` | open the current cell's HTML output in a browser |
| `:IpynbYankOutput` | copy the current cell's text output to the unnamed register |
| `:IpynbYankOutput!` | copy text output to the system clipboard |
| `:IpynbInterrupt` / `:IpynbRestart` | interrupt execution / restart the kernel |
| `:IpynbInfo` | show kernel information |

### saving and troubleshooting

`:w` saves cells and outputs. reopening restores results without rerunning code;
saved results are marked `[OLD]` until rerun. restarting the kernel keeps
displayed outputs but clears variables, so models and other Python state need
their own save/load code.

existing mappings are preserved. the shortcuts above assume the default
`<localleader>` of `\`; use your configured leader if different. modified enter
keys depend on terminal support, and `:Ipynb` always offers the same actions.

numpy scalars use book-style display: `np.uint8(7)` appears as `7`, without
changing the value or dtype. set `numpy_legacy_repr = false` to keep modern
numpy formatting. older saved outputs change when their cells run again.

use `:checkhealth ipynb` for setup problems, `:IpynbInstall` to repair the managed
Python environment and rebuild the Rust engine, and `:help ipynb` for custom environments. the notebook
runtime is local; hosted compute, collaboration and browser widgets are outside
this release.

The Rust engine manages notebook state, execution, and file writes. Notebook
cells still use their selected Python/Jupyter kernel. Jupytext conversion runs
in a persistent helper to preserve notebook metadata and formatting. See
`:help ipynb-rust` for builds and the Python compatibility backend.

## credits

includes the customised [molten](https://github.com/benlubas/molten-nvim) runtime,
derived from [magma](https://github.com/dccsillag/magma-nvim). uses jupytext,
jupyter, quarto-nvim, otter.nvim, treesitter and image.nvim.
[full credits and provenance](CREDITS.md). [gpl-3.0 licence](LICENSE).
