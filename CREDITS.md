# credits

## included source

`rplugin/python3/ipynb_runtime/` and `lua/ipynb/molten_*` derive from
[whitehades/molten-nvim](https://github.com/WhiteHades/molten-nvim/tree/e2094c8f28ed94a25176177b3be4ef1eeff65f8c),
commit `e2094c8f28ed94a25176177b3be4ef1eeff65f8c`, under gpl-3.0.

molten is maintained by ben lubas and contributors and derives from
[magma-nvim](https://github.com/dccsillag/magma-nvim) by daniel csillag and
contributors. their work provides kernel communication, cell execution,
output windows, persistence, and image integration. the original licence is
preserved in [license](LICENSE).

changes in this project include independent python, lua and command namespaces,
completed-cell output controls, terminal progress rendering, runtime directory
creation, optional numpy book-style display, shared-kernel cleanup, and safer
output persistence. the notebook setup and markdown
renderer were consolidated from whitehades' dotfiles and book editor integration.
these are modified sources, not an unchanged upstream release.

## installed dependencies

these projects remain separate packages. their source is not copied here.

| project | contribution | licence |
| --- | --- | --- |
| [jupytext.nvim](https://github.com/goerz/jupytext.nvim) by michael goerz | notebook conversion in neovim | mit |
| [quarto-nvim](https://github.com/quarto-dev/quarto-nvim) by posit and contributors | cell selection and execution routing | gpl-2.0-or-later |
| [otter.nvim](https://github.com/jmbuhr/otter.nvim) by jannik buhr and contributors | embedded python buffers and language services | mit |
| [nvim-treesitter](https://github.com/nvim-treesitter/nvim-treesitter) | parsers and syntax queries | apache-2.0 |
| [image.nvim](https://github.com/3rd/image.nvim) by andrei neculaesei and contributors | terminal images | mit |

python tools come from [jupyter](https://jupyter.org),
[jupytext](https://github.com/mwouts/jupytext),
[pynvim](https://github.com/neovim/pynvim), [numpy](https://numpy.org),
[matplotlib](https://matplotlib.org), and [pillow](https://python-pillow.org).
each retains its own licence. uv is an optional installer; neovim, lazy.nvim,
lazyvim, imagemagick and treesitter provide the editor and installation tooling.

render-markdown.nvim inspired the existing highlight integration. its groups
are reused when available, with native neovim highlight fallbacks otherwise.
