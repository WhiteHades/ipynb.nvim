# Jupytext in Neovim

Run `:IpynbJupytext` to choose an action. Use Tab to complete commands,
or run an action directly, such as `:IpynbJupytext sync`.

## Actions

```text
info     Show settings, paired paths, and formats
export   Save a copy in another format
pair     Set paired formats, such as ipynb,py:percent
unpair   Stop pairing and keep the files
sync     Choose which source to keep
pipe     Run a command and review its changes
check    Run a command to check the notebook
cli      Open the Jupytext command in a terminal
```

Choose a format in the picker and install any converter it requests.

## Project settings

Put settings in `jupytext.toml` or `[tool.jupytext]` in `pyproject.toml`.
For example, pair with a Python script and keep cell metadata:

```toml
formats = "ipynb,py:percent"
cell_metadata_filter = "all"
```

Use TOML for settings rather than executable `.jupytext.py` files.

## Saving and conflicts

`:w` saves the notebook and its paired files. If files differ or changed
elsewhere, use `sync` to choose a source. Pairs using external converters need
an initial sync. Export and pairing ask before overwriting files.

Unpairing applies to the current notebook. If a paired save partly fails,
check the error for files already saved before retrying.

Approve commands before running `pipe` or `check`. Review pipe changes, then
use `:w` to save them. Commands have a 20 second response limit. For longer
commands, use `cli`. Save or sync first and reload afterward, since CLI writes
bypass the editor's conflict checks.

## Upgrades

Run `:IpynbInstall`, restart Neovim, and check `:checkhealth ipynb`.
For custom helpers, update the `requirements.txt` packages yourself.
Jupytext 1.19.6 or newer is required.
