# Attribution

Every asset file in this repository, with its source and licence.

| File | Author / source | Licence |
|---|---|---|
| Sample data (`countries`, `cities`, `rivers`, `lakes` GeoJSON), embedded at build time from [rustgis-assets](https://github.com/opengeos/rustgis-assets/tree/main/data) | Natural Earth 1:110m, attributes trimmed | Public domain |

Icons are original vector drawings in code (`crates/ui-egui/src/icons.rs`). Fonts are not
bundled: the interface uses an installed system sans-serif when one is found, otherwise the
Ubuntu Light face that ships inside `epaint_default_fonts` (Ubuntu Font Licence 1.0).
