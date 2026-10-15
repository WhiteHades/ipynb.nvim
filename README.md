<p align="center"><img src="assets/logo.svg" width="112" alt="ipynb.nvim logo"></p>

# ipynb.nvim

[Jupyter](https://jupyter.org/) notebooks in [Neovim](https://neovim.io). Edit cells, run code, and view plots in your terminal.

[Latest release](https://github.com/WhiteHades/ipynb.nvim/releases) | [Neovim 0.12 or newer](https://neovim.io) | [GPL 3.0](LICENSE)

<p align="center"><a href="assets/notebook.png"><img src="assets/notebook.png" alt="Python cells, text output, and a plot in a fullscreen notebook" width="900"></a></p>

## Install

Requires Neovim 0.12 or newer. For [LazyVim](https://www.lazyvim.org/), add this plugin spec to `~/.config/nvim/lua/plugins/ipynb.lua`:

```lua
return {
  {
    "WhiteHades/ipynb.nvim",
    lazy = false,
    opts = {},
    dependencies = {
      "goerz/jupytext.nvim",
      { "quarto-dev/quarto-nvim", dependencies = { "jmbuhr/otter.nvim" } },
      { "nvim-treesitter/nvim-treesitter", build = ":TSUpdate" },
      { "3rd/image.nvim", opts = { processor = "magick_cli" } },
    },
  },
}
```

Run `:Lazy sync`, then `:IpynbInstall` and restart Neovim.
See [installation details](doc/install.md) for system requirements and image setup.

## LazyVim integrations

The required plugins below are declared in the spec above.

| Plugin | Use |
| --- | --- |
| [jupytext.nvim](https://github.com/goerz/jupytext.nvim) | Read and write notebook files. |
| [quarto.nvim](https://github.com/quarto-dev/quarto-nvim) | Required by the notebook setup. |
| [otter.nvim](https://github.com/jmbuhr/otter.nvim) | Provides embedded code buffers for language tools. |
| [nvim-treesitter](https://github.com/nvim-treesitter/nvim-treesitter) | Install the Markdown and Python parsers. |
| [image.nvim](https://github.com/3rd/image.nvim) | Render plots with the default image provider. |

The following integrations are optional and use your existing plugin settings.

| Plugin | Use |
| --- | --- |
| [render-markdown.nvim](https://github.com/MeanderingProgrammer/render-markdown.nvim) | Render notebook prose with your Markdown settings. |
| [conform.nvim](https://github.com/stevearc/conform.nvim) and [Neovim](https://neovim.io) LSP formatters | Format each code cell on save. |
| [nvim-lint](https://github.com/mfussenegger/nvim-lint) | Run configured linters and show diagnostics through Otter. |
| [snacks.nvim](https://github.com/folke/snacks.nvim) | Render floating plots; inline plots use [image.nvim](https://github.com/3rd/image.nvim). |
| [blink.cmp](https://github.com/saghen/blink.cmp) | Close completion menus when notebook insert mode ends. |
| [gitsigns.nvim](https://github.com/lewis6991/gitsigns.nvim) | Skip signs in notebook buffers. |
| [mini.hipatterns](https://github.com/nvim-mini/mini.hipatterns) | Keep inline color previews readable over cell backgrounds. |

## Use

1. Open `nvim example.ipynb` and use `:Ipynb` for notebook actions.
2. Save with `:w`; configured synchronous save hooks apply and outputs return on reopening.
3. Python runs in the nearest project `.venv`, prepared on first execution.
4. Use `zo` and `zc` for metadata; insert mode exposes the active cell's fences.

See [configuration and saving](doc/ipynb.txt) for settings and standalone environments.

[PyTorch](https://github.com/pytorch/pytorch) can use CUDA in your project kernel; notebook code selects GPU or CPU.

## Help

Run `:checkhealth ipynb` to check setup or `:IpynbInstall` to repair it.
See [Jupytext](https://github.com/mwouts/jupytext), [format help](doc/jupytext.md), [testing](native/README.md), [credits](CREDITS.md), or [report an issue](https://github.com/WhiteHades/ipynb.nvim/issues).
