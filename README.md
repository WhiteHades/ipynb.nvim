<p align="center"><img src="assets/logo.svg" width="112" alt="ipynb.nvim logo"></p>

<h1 align="center">ipynb.nvim</h1>

<p align="center"><a href="https://jupyter.org/">Jupyter</a> notebooks in <a href="https://neovim.io">Neovim</a>. Edit cells, run code, and view plots in your terminal.</p>

<p align="center">
  <a href="https://github.com/WhiteHades/ipynb.nvim/releases/latest"><img src="https://img.shields.io/github/v/release/WhiteHades/ipynb.nvim?label=latest%20release" alt="Latest release"></a>
  <a href="https://neovim.io"><img src="https://img.shields.io/badge/Neovim-0.12%2B-57A143?logo=neovim&logoColor=white" alt="Neovim 0.12 or newer"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/WhiteHades/ipynb.nvim?label=GPL-3.0" alt="GPL 3.0 license"></a>
  <a href="https://jupyter.org/"><img src="https://img.shields.io/badge/Jupyter-notebooks-F37626?logo=jupyter&logoColor=white" alt="Jupyter notebooks"></a>
</p>

<p align="center"><a href="assets/notebook.png"><img src="assets/notebook.png" alt="Python cells, text output, and a plot in a fullscreen notebook" width="900"></a></p>

## Install

Requires Neovim 0.12 or newer. Add this plugin spec to your [lazy.nvim](https://github.com/folke/lazy.nvim) configuration:

```lua
return {
  "WhiteHades/ipynb.nvim",
  lazy = false,
  opts = {},
}
```

Run `:Lazy sync`, then `:IpynbInstall` and restart Neovim.
See [installation details](doc/install.md) for system requirements and image setup.

## Use

1. Open `nvim example.ipynb` and use `:Ipynb` for notebook actions.
2. Save with `:w`; configured synchronous save hooks apply and outputs return on reopening.
3. Python runs in the nearest project `.venv`, prepared on first execution.
4. Use `zo` and `zc` for metadata; insert mode exposes the active cell's fences.

See [configuration and saving](doc/ipynb.txt) for settings and standalone environments.

[PyTorch](https://github.com/pytorch/pytorch) can use CUDA in your project kernel; notebook code selects GPU or CPU.

## Editor plugins

| Plugin | Integration |
| --- | --- |
| [Snacks quickfile](https://github.com/folke/snacks.nvim) | Preserves notebook highlighting during quickfile startup. |
| [render-markdown.nvim](https://github.com/MeanderingProgrammer/render-markdown.nvim) | Uses its configured renderer for notebook prose. |
| [blink.cmp](https://github.com/saghen/blink.cmp) | Keeps completion menus aligned with the active notebook insert session. |
| [conform.nvim](https://github.com/stevearc/conform.nvim) | Runs configured synchronous save formatters on code cells. |
| [nvim-lint](https://github.com/mfussenegger/nvim-lint) | Runs configured linters on save for notebook code cells. |
| [which-key.nvim](https://github.com/folke/which-key.nvim) | Shows notebook key descriptions in its popup. |

## Help

Run `:checkhealth ipynb` to check setup or `:IpynbInstall` to repair it.
See [Jupytext](https://github.com/mwouts/jupytext), [format help](doc/jupytext.md), [testing](native/README.md), [credits](CREDITS.md), or [report an issue](https://github.com/WhiteHades/ipynb.nvim/issues).
