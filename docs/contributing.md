# Contributing

Contributions are welcome. See [AGENTS.md](https://github.com/opengeos/RustGIS/blob/main/AGENTS.md)
for the conventions.

1. Fork the repository and create a branch.
2. Make your change with tests. Engine behaviour is a command in `crates/engine/src/cmd/`;
   UI-only commands (`ui.*`) live in `crates/ui-egui/src/lib.rs`.
3. Run `cargo xtask ci` and `pre-commit run --all-files`.
4. Open a pull request. Rust CI runs on Linux by default. A maintainer can label it `ci:windows`,
   `ci:macos` or `ci:full` to test those platforms before merging; pushes to `main` test all three.
   `ci:full` also builds the release apps. The nightly workflow tests and release-builds all three
   when there are new commits, and opens an issue on failure.
5. The docs workflow builds a site and web-app preview. A first-time fork contributor's preview
   needs a maintainer to add `preview` or re-run the workflow; returning contributors' previews
   publish automatically.

A maintainer's `automerge` label approves and squash-merges a pull request after its required
checks pass. Pushing again removes the label and disables auto-merge; removing the label also
disables it.

## Developer checks

`cargo xtask ci` runs `cargo fmt --all --check`,
`cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace -q` and
`cargo xtask layers`. The pre-commit hooks also check TOML, YAML, JSON, whitespace and large files.
CI separately builds `rustgis-web` for `wasm32-unknown-unknown` and runs
`cargo deny --locked check licenses` against `deny.toml`.

Other workspace tasks:

| Task | Purpose |
| --- | --- |
| `cargo xtask layers` | Check crate dependency direction and keep UI dependencies out of lower layers |
| `cargo xtask notices` | Regenerate the shipped `THIRD-PARTY-LICENSES.txt` files after changing dependencies; requires `cargo-about` |
| `cargo xtask notices --check` | Check the notices without writing them; stale notices warn in CI but block releases |
| `cargo xtask web-pwa <dir>` | Generate the service worker in a Trunk build output directory, such as `dist/web` |

Interface strings must be translatable: use `tl!("…")` for literals and
`i18n::fmt("… {name} …", &[("name", value)])` for messages with values. New strings can fall back
to English until the catalogs are updated; see [Languages](i18n.md).

## Test data

Generate small datasets during tests instead of committing fixture files. The shared generator
in `tests/support/ogc.rs` builds OGC landing pages, collection lists and GeoJSON in memory;
`tests/support/ogc_server.rs` serves them locally for engine and UI tests. The sorting smoke
test similarly generates its KML, KMZ, CSV and GeoJSON inputs. These run automatically with
`cargo test --workspace --locked` and `cargo xtask ci`, without a separate setup step or a
public service. Extend these generators when adding cases.

## Documentation

The site is built with [Zensical](https://zensical.org):

```sh
pip install -r requirements_docs.txt
zensical serve        # http://localhost:8000
```

Images live in [rustgis-assets](https://github.com/opengeos/rustgis-assets) and are linked by
URL; don't commit images to this repository.

The screenshots are rendered by the app itself. From the repository root, run
`crates/ui-egui/examples/shots/render.sh`; it needs ImageMagick (`magick`) and network access for
tiles, plugins and the sample DEM. It runs the `ui_shot` scripts beside it and writes WebP files
to `target/shots/`; copy them to `images/` in rustgis-assets. The `assistant` shot asks a real
model, so set `RUSTGIS_ASSISTANT_PROVIDER` and the provider's key first, or it shows an empty
pane (see [AI Assistant](user-guide/assistant.md#choosing-a-model)).

## Releases

1. Bump `version` in the workspace `Cargo.toml` and merge it.
2. Refresh third-party notices if dependencies changed (`cargo xtask notices`).
3. Run the **Release dry run** workflow, which checks the version and builds the desktop/CLI
   packages without publishing anything:
   `gh workflow run release-dry-run.yml -R opengeos/RustGIS --ref main -f version=<version>`
4. When it passes, publish a GitHub release tagged `v<version>` (the tag must match `Cargo.toml`).

The **Release** workflow then builds and attaches the Windows installers, zip and MSIX, the
macOS DMGs, the Linux tarballs, `.deb` and `.rpm` packages and AppImages, and `SHA256SUMS`. To try the packaging locally:

```sh
cargo build --release --locked -p rustgis -p rustgis-cli
packaging/package.sh <version>  # writes dist/ for the host platform
```
