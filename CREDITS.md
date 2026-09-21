# credits

## molten and magma

thanks to ben lubas and contributors for
[molten-nvim](https://github.com/benlubas/molten-nvim), and daniel csillag and
contributors for [magma-nvim](https://github.com/dccsillag/magma-nvim), which
molten grew out of.

the python runtime in `rplugin/python3/ipynb_runtime/` and the
`lua/ipynb/molten_*` files come from [my molten fork](https://github.com/WhiteHades/molten-nvim/tree/e2094c8f28ed94a25176177b3be4ef1eeff65f8c),
at commit `e2094c8f28ed94a25176177b3be4ef1eeff65f8c`. i've modified them for
this plugin. the original gpl-3.0 licence is included in [LICENSE](LICENSE).

the notebook setup and markdown renderer came from my dotfiles and book editor
integration. i reuse render-markdown.nvim's highlight groups when available
and fall back to neovim's otherwise.

## dependencies

these projects are installed separately and keep their own licences.

| project | used for | licence |
| --- | --- | --- |
| [jupytext.nvim](https://github.com/goerz/jupytext.nvim) by michael goerz | notebook conversion | mit |
| [quarto-nvim](https://github.com/quarto-dev/quarto-nvim) by posit and contributors | selecting and running cells | gpl-2.0-or-later |
| [otter.nvim](https://github.com/jmbuhr/otter.nvim) by jannik buhr and contributors | language support inside cells | mit |
| [nvim-treesitter](https://github.com/nvim-treesitter/nvim-treesitter) | parsing and syntax highlighting | apache-2.0 |
| [image.nvim](https://github.com/3rd/image.nvim) by andrei neculaesei and contributors | plots in the terminal | mit |

the python side uses [jupyter](https://jupyter.org),
[jupytext](https://github.com/mwouts/jupytext),
[pynvim](https://github.com/neovim/pynvim), [numpy](https://numpy.org),
[matplotlib](https://matplotlib.org), and [pillow](https://python-pillow.org).
