<div align="center">

<pre>
    _                   _&#32;&#32;&#32;&#32;&#32;&#32;
   (_) _ __  _   _ _ __ | |__&#32;&#32;
   | || '_ \| | | | '_ \| '_ \&#32;
   | || |_) | |_| | | | | |_) |
   |_|| .__/ \__, |_| |_|_.__/&#32;
      |_|    |___/       .nvim&#32;
</pre>

<h1>ipynb.nvim</h1>
<p>Jupyter notebooks in Neovim. Edit cells, run code, and view plots in your terminal.</p>

<p>
<a href="https://github.com/WhiteHades/ipynb.nvim/releases"><img src="https://img.shields.io/github/v/release/WhiteHades/ipynb.nvim?style=flat-square&amp;cacheSeconds=300" alt="Latest release"></a>
<a href="https://neovim.io"><img src="https://img.shields.io/badge/Neovim-0.12%2B-57a143?style=flat-square&amp;logo=neovim&amp;logoColor=white" alt="Neovim 0.12 or newer"></a>
<a href="https://github.com/WhiteHades/ipynb.nvim/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-GPL--3.0-blue?style=flat-square" alt="GPL-3.0 license"></a>
</p>

<p><a href="#install">Install</a> | <a href="#use">Use</a> | <a href="#help">Help</a></p>

<a href="https://github.com/WhiteHades/ipynb.nvim/blob/main/assets/notebook.png"><img src="https://raw.githubusercontent.com/WhiteHades/ipynb.nvim/main/assets/notebook.png" alt="A notebook running in Neovim" width="900"></a>

</div>

## Install

Requires Neovim 0.12 or newer. For LazyVim, create
`~/.config/nvim/lua/plugins/ipynb.lua`:

```lua
return {
  { "WhiteHades/ipynb.nvim", lazy = false, opts = {} },
}
```

With lazy.nvim, add the same plugin spec. Run `:Lazy sync` and restart Neovim
after installation. Missing tools need confirmation and may need sudo.

## Use

Open a notebook with `nvim example.ipynb`. Notebook prose uses your existing
render-markdown.nvim configuration when available. Cell labels sit in the code
border, leaving your normal line numbers and relative numbers unchanged.
Metadata uses a native fold, so moving down skips the whole summary. Code and
output share a theme-derived background, without a separate focused-cell fill.

```text
:Ipynb or \n      Open the action menu
\r                Run the current cell or visual selection
Ctrl+Enter        Run the current cell
Shift+Enter       Run and move to the next code cell
\R                Run all cells
\a                Run this cell and the cells above
\b                Add a code cell below
\m                Add a Markdown cell below
\s                Interrupt execution
\i                Restart the kernel and clear variables
\o                Open full output
\O                Close full output
\x                Enlarge first image or open HTML in a browser
:w                Save code and outputs
:IpynbJupytext    Export, pair, and sync files
:set mouse=a      Enable clicking plots
Click a plot      Enlarge it
Click More lines  Open the complete output or traceback
q or Escape       Close the image or output window

Default <localleader>: \
Enter key combinations depend on your terminal.

Saved outputs return when you reopen the notebook.
Python notebooks use the project's .venv, set up on first execution.
Set vim.g.ipynb_animations = false in your config to stop running-cell spinners.
```

Saving uses your configured synchronous save hooks for each cell's language,
including Conform and LSP formatting. Your existing autosave settings apply.
Configured nvim-lint tools receive embedded code, and Otter maps their diagnostics
back to the notebook. File-only linters require Otter's `buffers.write_to_disk`.
The plugin does not select or install formatters or linters.

## Help

Use `:checkhealth ipynb` to check setup and `:IpynbInstall` to repair or update it.
Run `:help ipynb` for shortcuts, environments, and kernel settings.

See [installation](doc/install.md), [Jupytext](doc/jupytext.md),
[testing](native/README.md), or [report an issue](https://github.com/WhiteHades/ipynb.nvim/issues).

See [credits](CREDITS.md) and the [GPL 3.0 license](LICENSE).
