# Testing

From the repository root, run:

```sh
IPYNB_TEST_PYTHON=/path/to/helper/bin/python \
  cargo test --release --locked --manifest-path native/Cargo.toml
```

## Editor check

Set the following variables to check the editor with installed plugins.

```text
IPYNB_TEST_PLUGINS   Installed Neovim plugin directory
IPYNB_PYRIGHT        Pyright executable
IPYNB_TEST_PYTHON    Helper Python
IPYNB_PROJECT_PYTHON Another environment's Python
IPYNB_TEST_SITE      Parser runtime directory, if stored elsewhere
```

Both Python environments need `ipykernel`. Build and run from the repository root:

```sh
cargo build --release --locked --manifest-path native/Cargo.toml
cargo test --manifest-path native/Cargo.toml --test notebook_view --test project_workflow -- --ignored
```

With ImageMagick 7, also check plot scaling and transparent corners:

```sh
cargo test --manifest-path native/Cargo.toml --test image_kitty -- --include-ignored
```
