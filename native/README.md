# Engine

`engine/` owns cells, outputs, and notebook state. `kernel/` owns local processes,
signed messages, remote connections, and worker cleanup.
`notebook.rs` owns atomic JSON saves, and `kernelspec.rs` finds installed kernels.

The converter starts on demand for Jupytext conversion and optional renderers.
Lua handles Neovim windows, mappings, and buffer events.

Run core checks with `IPYNB_TEST_PYTHON=/path/to/helper/bin/python cargo test --release --locked`.
