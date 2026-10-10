# Contributing

Contributions are welcome. See [AGENTS.md](https://github.com/opengeos/RustGIS/blob/main/AGENTS.md)
for the conventions.

1. Fork the repository and create a branch.
2. Make your change with tests. New behaviour is a command in `crates/engine/src/cmd/`.
3. Run `cargo xtask ci` and `pre-commit run --all-files`.
4. Open a pull request. CI runs on Linux, macOS and Windows, and the docs build a preview.

## Documentation

The site is built with [Zensical](https://zensical.org):

```sh
pip install -r requirements_docs.txt
zensical serve        # http://localhost:8000
```

Images live in [rustgis-assets](https://github.com/opengeos/rustgis-assets) and are linked by
URL; don't commit images to this repository.
