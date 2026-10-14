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

<p><a href="#install">Install</a> | <a href="#use">Use</a> | <a href="#help">Help</a></p>

<a href="https://github.com/WhiteHades/ipynb.nvim/blob/main/assets/notebook.png"><img src="https://raw.githubusercontent.com/WhiteHades/ipynb.nvim/main/assets/notebook.png" alt="A notebook running in Neovim" width="900"></a>

</div>

## Install

Use Neovim 0.12 or newer. For LazyVim, create
`~/.config/nvim/lua/plugins/ipynb.lua`:

```lua
return {
  { "WhiteHades/ipynb.nvim", lazy = false, opts = {} },
}
```

For plain lazy.nvim, add the same spec to your setup. Run `:Lazy sync`, restart
Neovim after installation finishes, then open `nvim example.ipynb`.
A missing or empty file opens with one empty code cell.

The build and `:IpynbInstall` check host tools before setting up the helper,
parsers, and Rust engine. Missing tools open a terminal with an install plan
and a confirmation prompt. System packages may need sudo. A headless install
stops with a shell command instead of waiting for input. If all required tools
are ready, no prerequisite installation runs.

New helper environments use Python 3.15.0 through uv, separate from project environments.
The Rust engine and parsers build from source and need compiler tools.
Rust is the only execution backend. The Python execution backend and optional
jiter decoder have been removed. Existing `backend = "rust"` settings still work;
remove `backend = "python"` from older configurations.
The small Python helper still supplies Jupytext conversion and kernel discovery.
Its direct requirements are `jupytext` and `ipykernel`, which install the required
Jupyter and nbformat libraries. Plotting packages belong in each notebook project,
not the editor's helper environment. Neovim's Python host is not used or changed.
See [requirements and other setups](doc/install.md) for the standalone checker,
supported package managers, and installation without a plugin manager.

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

Python notebooks use the nearest Python project's `.venv` for execution and
Pyright completion. On the first run, the plugin uses `uv` to set up a missing
environment and install `ipykernel` if needed. A standalone notebook gets
`pyproject.toml`, `uv.lock`, and `.venv` in its own directory, even inside an
unrelated Git repository. An existing environment with `ipykernel` stays unchanged.

First setup needs `uv` and may need network access. If setup fails, the plugin
reports the error without using global execution packages. Set
`opts = { auto_setup = false }` to manage the environment yourself.
The editor's helper Python stays separate from project dependencies.

Opening a notebook restores saved results without
starting a kernel. The kernel starts when you first run code. `:IpynbInfo` shows
its interpreter, working directory, and selection reason. Use `:IpynbInit` to
choose another registered or shared kernel. Named, non-Python, and remote kernel
choices remain available.
See `:help ipynb-options` for overrides. If the Rust engine is missing, notebook
operations report an error with instructions to run `:IpynbInstall`.

## Help

- `:checkhealth ipynb` checks setup; `:IpynbInstall` repairs the runtime.
- `:help ipynb` covers configuration, commands, and image controls.
- [Installation details](doc/install.md), [releases](https://github.com/WhiteHades/ipynb.nvim/releases), and [issue reports](https://github.com/WhiteHades/ipynb.nvim/issues).

Built on the Molten interface and the Jupyter ecosystem, with a Rust engine.
[Credits and provenance](CREDITS.md), [GPL-3.0](LICENSE).

## Testing

Run the Rust checks, including installer checks that use isolated command
substitutes and headless Neovim without installing system packages:

```sh
cargo test --release --locked --manifest-path native/Cargo.toml
```

`native/tests/project_workflow.rs` is an ignored Rust E2E test that uses installed
Neovim plugins. Set `IPYNB_TEST_PLUGINS` to their directory and `IPYNB_PYRIGHT`
to the Pyright executable. Use `IPYNB_TEST_PYTHON` for the helper environment's
Python and `IPYNB_PROJECT_PYTHON` for another environment's Python.
Both environments need `ipykernel`. If parsers live elsewhere, set
`IPYNB_TEST_SITE` to their runtime directory. Build the Rust engine first, then run:

```sh
cargo test --manifest-path native/Cargo.toml --test project_workflow -- --ignored
```
