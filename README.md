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
<a href="https://github.com/WhiteHades/ipynb.nvim/actions/workflows/tests.yml"><img src="https://github.com/WhiteHades/ipynb.nvim/actions/workflows/tests.yml/badge.svg" alt="Tests"></a>
<a href="https://neovim.io"><img src="https://img.shields.io/badge/Neovim-0.12%2B-57a143?style=flat-square&amp;logo=neovim&amp;logoColor=white" alt="Neovim 0.12 or newer"></a>
<a href="https://github.com/WhiteHades/ipynb.nvim/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-GPL--3.0-blue?style=flat-square" alt="GPL-3.0 license"></a>
</p>

<p><a href="#install">Install</a> · <a href="#use">Use</a> · <a href="#help">Help</a></p>

<a href="https://github.com/WhiteHades/ipynb.nvim/blob/main/assets/notebook.png"><img src="https://raw.githubusercontent.com/WhiteHades/ipynb.nvim/main/assets/notebook.png" alt="A notebook running in Neovim" width="900"></a>

</div>

## Install

Check prerequisites and install missing tools with curl and Bash:

```sh
ipynb_setup=$(curl -fsSL https://raw.githubusercontent.com/WhiteHades/ipynb.nvim/main/scripts/prerequisites.sh) &&
  bash -c "$ipynb_setup" -- --install
```

The helper previews changes before installing. Omit `--install` to check only.
Supports apt, dnf, pacman, and Homebrew; pacman also upgrades the system.
See [requirements and other setups](doc/install.md) if anything remains missing.

For LazyVim, create `~/.config/nvim/lua/plugins/ipynb.lua`:

```lua
return {
  { "WhiteHades/ipynb.nvim", lazy = false, opts = {} },
}
```

For plain lazy.nvim, add the same spec to your setup. Run `:Lazy sync`, restart
Neovim, then open `nvim example.ipynb`. Notebook tools install automatically;
a new path creates a blank notebook.

## Use

Put the cursor inside a cell and run `:Ipynb` for the action menu.
These shortcuts use the default `<localleader>` of `\`:

| Key | Action |
| --- | --- |
| `\r` / ctrl-enter | Run cell; `\r` also runs a visual selection |
| shift-enter | Run and move to the next code cell |
| `\R` / `\a` | Run all / run this cell and above |
| `\b` / `\m` | Insert a code / Markdown cell below |
| `\o` / `\O` | Open / close full output |
| `\x` | Enlarge the cell's first image |
| `\s` / `\i` | Interrupt / restart the kernel |

Enable `:set mouse=a` and **click a plot to enlarge it** inside the terminal.
Use `q` or escape to close the viewer or output window. Existing mappings take
precedence; modified enter keys depend on your terminal.

`:w` saves cells and outputs. Reopening restores results without rerunning code.
Restarting the kernel keeps displayed outputs but clears Python variables.

## Help

- `:checkhealth ipynb` checks setup; `:IpynbInstall` repairs the runtime.
- `:help ipynb` covers configuration, commands, and image controls.
- [Installation details](doc/install.md) · [Releases](https://github.com/WhiteHades/ipynb.nvim/releases) · [Report an issue](https://github.com/WhiteHades/ipynb.nvim/issues)

Built on the customised Molten runtime and the Jupyter ecosystem.
[Credits and provenance](CREDITS.md) · [GPL-3.0](LICENSE)
