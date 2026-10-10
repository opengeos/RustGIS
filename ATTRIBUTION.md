# Attribution

Every asset file in this repository, with its source and licence.

| File | Author / source | Licence |
|---|---|---|
| `assets/data/countries.geojson` | Natural Earth, 1:110m Admin 0 – Countries (attributes trimmed) | Public domain |
| `assets/data/cities.geojson` | Natural Earth, 1:110m Populated Places (simple) | Public domain |
| `assets/data/rivers.geojson` | Natural Earth, 1:110m Rivers + lake centerlines | Public domain |
| `assets/data/lakes.geojson` | Natural Earth, 1:110m Lakes | Public domain |
| `docs/images/*.png` | Screenshots of RustGIS itself (`ui_shot` example) | Apache-2.0 |

Icons are original vector drawings in code (`crates/ui-egui/src/icons.rs`). Fonts are not
bundled: the interface uses an installed system sans-serif when one is found, otherwise the
Ubuntu Light face that ships inside `epaint_default_fonts` (Ubuntu Font Licence 1.0).
