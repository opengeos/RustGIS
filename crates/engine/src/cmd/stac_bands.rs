//! `stac.addBands`: a band combination of a STAC item as one multi-band raster layer.
//!
//! Sentinel-2 and Landsat publish one file per band, so a colour image is made from three files.
//! This reads the window of each (the view or a box, in place by range, as `layer.addUrl` with a
//! `bbox` does), puts them on one grid (the finest of the three, by nearest cell) and keeps them as
//! one raster of three bands drawn as red, green and blue. Scale and offset from the catalog are
//! applied, so the values are what the bands measure (reflectance), and a cell with no value in
//! any of the three is no data in all of them. The band names and which combinations an item
//! offers are [`bands`](crate::render::rustgis_tiles::stac::bands)'s.
//!
//! It makes its raster on another thread from the app, with Cancel ([`super::workflows`]).

use serde_json::Value;

use super::workflows::Work;
use crate::{CmdResult, CommandSpec as C, Session};

pub fn commands() -> Vec<C> {
    vec![C::new("stac.addBands", "Add STAC Bands", "Home › Add › Web Data › STAC Search", add_bands).roles(&[]).effects(crate::policy::Effects::NONE.with(crate::policy::Effect::Network)).view().params(
        r#"{"item": a STAC item (from stac.search), "combination"?: "trueColor"|"colorInfrared"|"shortwaveInfrared"|"falseColorUrban"|"agriculture"|"geology"|"bathymetric" (default trueColor), "red"?, "green"?, "blue"?: band names (red, nir, swir16… or an asset's key) used instead of a combination, "bbox"?: [west, south, east, north] in degrees | "view" (default), "cellSize"?: the coarsest overview whose cells are at most this size is read (default: about 2,048 cells across the box), "name"?: text, "ran"?: (internal: a raster the app has worked out)}"#,
    ),
    C::new("stac.addIndex", "Add STAC Index", "Home › Add › Web Data › STAC Search", add_index).roles(&[]).effects(crate::policy::Effects::NONE.with(crate::policy::Effect::Network)).view().params(
        r#"{"item": a STAC item (from stac.search), "index": "ndvi"|"ndwi"|"savi"|"nbr"|"ndbi"|"evi" (the Indices section's formulas, on the item's bands: NDVI nir and red, NDWI green and nir, SAVI nir and red, NBR nir and swir22, NDBI swir16 and nir, EVI nir, red and blue; their scale and offset applied), "bbox"?: [west, south, east, north] in degrees | "view" (default), "cellSize"?: the coarsest overview whose cells are at most this size is read (default: about 2,048 cells across the box), "name"?: text, "ran"?: (internal: a raster the app has worked out)} → a raster layer of one band"#,
    )]
}

fn add_bands(s: &mut Session, p: &Value) -> CmdResult {
    super::workflows::made_or_ran(s, p, "stac.addBands")
}

fn add_index(s: &mut Session, p: &Value) -> CmdResult {
    super::workflows::made_or_ran(s, p, "stac.addIndex")
}

/// The browser has no thread to wait for several files on.
#[cfg(target_arch = "wasm32")]
const IN_THE_BROWSER: &str = "band combinations are made on the desktop and the command line for now";

/// What `stac.addBands` reads of the session (the item given, the view), and the work that reads
/// the three files and makes the raster.
pub(crate) fn bands_work(s: &Session, p: &Value) -> Result<Work, String> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (s, p);
        Err(IN_THE_BROWSER.into())
    }
    #[cfg(not(target_arch = "wasm32"))]
    desktop::work(s, p)
}

/// What `stac.addIndex` reads of the session (the item given, the view), and the work that reads
/// the bands' files and makes the index.
pub(crate) fn index_work(s: &Session, p: &Value) -> Result<Work, String> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (s, p);
        Err(IN_THE_BROWSER.replace("band combinations are", "indices are"))
    }
    #[cfg(not(target_arch = "wasm32"))]
    desktop::index(s, p)
}

#[cfg(not(target_arch = "wasm32"))]
mod desktop {
    use std::collections::BTreeSet;
    use std::sync::atomic::{AtomicBool, Ordering};

    use rustgis_data::raster::Raster;
    use rustgis_map::{Ramp, RasterLayer, RasterRenderer, Stretch};
    use serde_json::{Value, json};

    use super::super::layer::{WindowAsked, WindowRaster};
    use super::super::ps;
    use super::super::workflows::{Made, Then, Work};
    use crate::Session;
    use crate::render::rustgis_tiles::stac::{self, bands as sb};

    /// A window's cells across, when no cell size is given: its overview is the one nearest this.
    const ACROSS: f64 = 2048.0;

    /// A kilometre of a degree of latitude, in metres.
    const DEGREE_M: f64 = 111_320.0;

    /// The item given and the bands its GeoTIFF files offer.
    fn item_of(p: &Value) -> Result<(stac::Item, Vec<sb::Band>), String> {
        let item = match p.get("item").filter(|v| v.is_object()) {
            Some(v) => stac::item_from_json(v, None).ok_or("item isn't a STAC item")?,
            None => return Err("give item: a STAC item, as stac.search answers".into()),
        };
        let all = sb::bands(&item);
        if all.is_empty() {
            return Err(format!("{}: it has no GeoTIFF asset to make a band combination from", item.id));
        }
        Ok((item, all))
    }

    /// What reading the bands `wanted` takes: each band found (or an error naming what the item has),
    /// each file's address (checked as `stac.addAsset` checks it), and the window and cell size.
    fn plan(
        s: &Session,
        p: &Value,
        item: &stac::Item,
        all: &[sb::Band],
        wanted: &[String],
    ) -> Result<(Vec<sb::Band>, Vec<String>, [f64; 4], f64), String> {
        let mut picked = Vec::with_capacity(wanted.len());
        for name in wanted {
            let band = sb::find(all, name).ok_or_else(|| {
                let cut = sb::cut_note(item).map(|n| format!("; {n}")).unwrap_or_default();
                format!("{}: no band '{name}' (it has {}){cut}", item.id, names_of(all))
            })?;
            picked.push(band.clone());
        }
        let mut hrefs = Vec::with_capacity(picked.len());
        for band in &picked {
            hrefs.push(item.asset_href(Some(&band.asset))?.1);
        }
        let mut asked = p.clone();
        if asked.get("bbox").is_none_or(Value::is_null) {
            asked["bbox"] = json!("view");
        }
        let bbox = super::super::layer::window_box_of(s, &asked)?;
        let cell = match p.get("cellSize").filter(|v| !v.is_null()) {
            None => auto_cell(bbox),
            Some(v) => v.as_f64().filter(|c| c.is_finite() && *c > 0.0).ok_or("cellSize is a size above 0")?,
        };
        Ok((picked, hrefs, bbox, cell))
    }

    pub(super) fn work(s: &Session, p: &Value) -> Result<Work, String> {
        let (item, all) = item_of(p)?;
        // The three bands: a combination, or the three named.
        let named = ["red", "green", "blue"].map(|k| ps(p, k).map(str::trim).filter(|v| !v.is_empty()));
        // One or the other: a combination given with the three bands would be dropped without a word.
        if named.iter().any(Option::is_some) && ps(p, "combination").is_some_and(|c| !c.trim().is_empty()) {
            return Err("give a combination, or red, green and blue, not both".into());
        }
        let (label, wanted): (String, [String; 3]) = if named.iter().any(Option::is_some) {
            let [Some(r), Some(g), Some(b)] = named else { return Err("give red, green and blue together, or a combination".into()) };
            (format!("{r}-{g}-{b}"), [r.to_string(), g.to_string(), b.to_string()])
        } else {
            let id = ps(p, "combination").map(str::trim).filter(|v| !v.is_empty()).unwrap_or("trueColor");
            let c = sb::COMBINATIONS.iter().find(|c| c.id.eq_ignore_ascii_case(id)).ok_or_else(|| {
                let ids: Vec<&str> = sb::COMBINATIONS.iter().map(|c| c.id).collect();
                format!("unknown combination '{id}' (the combinations are {})", ids.join(", "))
            })?;
            (c.name.to_string(), c.channels.map(str::to_string))
        };
        let (picked, hrefs, bbox, cell) = plan(s, p, &item, &all, &wanted)?;
        let name = ps(p, "name").map(str::trim).filter(|n| !n.is_empty()).map_or_else(|| format!("{} {label}", item.id), str::to_string);
        let channels = [wanted[0].clone(), wanted[1].clone(), wanted[2].clone()];
        Ok(Box::new(move || {
            rustgis_data::progress::check().map_err(|e| e.to_string())?;
            let (made, notes) = read_and_stack(&name, &channels, &picked, &hrefs, bbox, cell)?;
            made_saying(made, notes, json!({"bands": channels, "bbox": bbox}))
        }))
    }

    /// `stac.addIndex`: the bands an index reads, read as a combination's are (the item's files, by
    /// range, over the window), and the index made from them as one band of numbers, by the
    /// Indices section's formula. The band math is one pass over the window with no Cancel check
    /// inside it (the window is bounded by the cell size); Cancel is looked at before each file.
    pub(super) fn index(s: &Session, p: &Value) -> Result<Work, String> {
        let (item, all) = item_of(p)?;
        let id = ps(p, "index").map(str::trim).filter(|v| !v.is_empty()).ok_or_else(|| {
            let ids: Vec<&str> = sb::INDICES.iter().map(|i| i.id).collect();
            format!("give index: {}", ids.join(", "))
        })?;
        let ix = sb::INDICES.iter().find(|i| i.id.eq_ignore_ascii_case(id)).ok_or_else(|| {
            let ids: Vec<&str> = sb::INDICES.iter().map(|i| i.id).collect();
            format!("unknown index '{id}' (the indices are {})", ids.join(", "))
        })?;
        let wanted: Vec<String> = ix.channels.iter().map(|c| (*c).to_string()).collect();
        let (picked, hrefs, bbox, cell) = plan(s, p, &item, &all, &wanted)?;
        // Where each input is in the bands read, for the formula (the ones it doesn't use are never named).
        let at = |names: &[&str]| ix.channels.iter().position(|c| names.contains(c)).map_or(1, |i| i as u64 + 1);
        let expr = super::super::imagery::index_expression(
            ix.id,
            at(&["blue"]),
            at(&["green"]),
            at(&["red"]),
            at(&["nir"]),
            at(&["swir16", "swir22"]),
        )?;
        let name = ps(p, "name")
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map_or_else(|| format!("{} {}", item.id, ix.id.to_ascii_uppercase()), str::to_string);
        let id = ix.id;
        Ok(Box::new(move || {
            rustgis_data::progress::check().map_err(|e| e.to_string())?;
            let (stacked, notes) = read_and_stack(&name, &wanted, &picked, &hrefs, bbox, cell)?;
            let band = rustgis_analysis::rasterops::band_math(&stacked, &expr)?;
            rustgis_data::progress::check().map_err(|e| e.to_string())?;
            let raster = rustgis_analysis::rasterops::derive(&stacked, &name, vec![band])?;
            let then = noted(raster.name.clone(), notes);
            let bytes = rustgis_data::raster::geotiff::write_all(&raster).map_err(|e| e.to_string())?;
            let layer_name = raster.name.clone();
            let mut layer = RasterLayer::from_bytes(raster, format!("{layer_name}.tif"), bytes);
            // Drawn in colour over the range the index means something in, not grey over this window's own.
            let (ramp, min, max) = index_look(id);
            layer.style.renderer = RasterRenderer::Stretch { band: 0, ramp, invert: false };
            layer.style.stretch = Stretch::Custom { min, max, bands: None };
            Ok(super::super::workflows::made_layer(
                layer_name,
                layer,
                Some(json!({"index": id, "bands": wanted, "bbox": bbox})),
                Some(then),
            ))
        }))
    }

    /// The colours an index is drawn in and the values they run between (low to high): greens for
    /// the vegetation indices, blues for water, red to blue for a burn ratio (burnt to healthy), and
    /// blue to red for the built-up index. The range is the index's own, so two scenes compare.
    pub(super) fn index_look(id: &str) -> (Ramp, f64, f64) {
        match id {
            "ndwi" => (Ramp::Lagoon, -0.5, 0.5),
            "nbr" => (Ramp::Diverging, -0.5, 0.5),
            "ndbi" => (Ramp::Thermal, -0.5, 0.5),
            "evi" => (Ramp::Forest, 0.0, 1.0),
            _ => (Ramp::Forest, -0.1, 0.9),
        }
    }

    /// The names a bands list offers, for an error: common names, else keys, each once.
    fn names_of(all: &[sb::Band]) -> String {
        let mut seen = BTreeSet::new();
        let names: Vec<&str> = all
            .iter()
            .map(|b| if b.common.is_empty() { b.asset.as_str() } else { b.common.as_str() })
            .filter(|n| seen.insert(*n))
            .collect();
        // At most 24 are listed, and the list says when there are more.
        let more = if names.len() > 24 { ", …" } else { "" };
        format!("{}{more}", names.iter().take(24).copied().collect::<Vec<_>>().join(", "))
    }

    /// The cell size that reads a box about [`ACROSS`] cells across: the overview at least that
    /// coarse, or the full image of a window small enough for it.
    pub(super) fn auto_cell(bbox: [f64; 4]) -> f64 {
        let [west, south, east, north] = bbox;
        let mid = ((south + north) / 2.0).to_radians().cos().abs().max(0.01);
        let (w, h) = ((east - west).abs() * DEGREE_M * mid, (north - south).abs() * DEGREE_M);
        (w.max(h) / ACROSS).max(0.001)
    }

    /// A window of the file at `url`, every band, read on a thread of its own while this one looks
    /// at the run's Cancel and stops the read once it is set. A thread the system refuses costs
    /// only that: the read runs here, Cancel looked at once it is in.
    fn read_window(url: &str, bbox: [f64; 4], cell: f64, bands: usize) -> Result<WindowRaster, String> {
        let asked = WindowAsked::of(url, bbox, Some(cell), bands);
        let stop = AtomicBool::new(false);
        let w = std::thread::scope(|scope| {
            let reader = std::thread::Builder::new()
                .name("STAC band window".into())
                .spawn_scoped(scope, || super::super::layer::read_window_raster(&asked, &stop));
            let Ok(reader) = reader else { return super::super::layer::read_window_raster(&asked, &stop) };
            while !reader.is_finished() {
                if rustgis_data::progress::check().is_err() {
                    stop.store(true, Ordering::Relaxed);
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            reader.join().unwrap_or_else(|_| Err(format!("{url}: the read failed unexpectedly")))
        })?;
        rustgis_data::progress::check().map_err(|e| e.to_string())?;
        Ok(w)
    }

    /// The three windows, read, and put on one grid: the raster of three bands, and what to say.
    fn read_and_stack(
        name: &str,
        channels: &[String],
        picked: &[sb::Band],
        hrefs: &[String],
        bbox: [f64; 4],
        cell: f64,
    ) -> Result<(Raster, Vec<String>), String> {
        // One read for a file, whichever of its bands are wanted.
        let mut windows: Vec<(String, WindowRaster)> = Vec::new();
        let mut notes = Vec::new();
        for (i, band) in picked.iter().enumerate() {
            let href = hrefs.get(i).ok_or("a band has no address")?;
            if windows.iter().any(|(h, _)| h == href) {
                continue;
            }
            // A Planetary Computer file is signed as it is read, not when the item was found.
            let signed = stac::sign_blocking(href)?;
            // Every band of the file is read, so the count asked is the file's, not the band's place.
            // The cost grows with the file's band count (a 13-band file is about 13 times the bytes
            // and memory of the one band wanted); the values read are bounded by the window's own
            // limit, and a window reader that picks bands would spare it.
            let w = read_window(&signed, bbox, cell, band.in_file.max(band.index) as usize)?;
            let who = channels.get(i).map_or("band", String::as_str);
            notes.extend(w.notes().into_iter().map(|n| format!("{who}: {n}")));
            windows.push((href.clone(), w));
        }
        let sources: Vec<(&Raster, &sb::Band)> = picked
            .iter()
            .enumerate()
            .map(|(i, band)| {
                let href = hrefs.get(i).ok_or("a band has no address")?;
                let w = windows.iter().find(|(h, _)| h == href).map(|(_, w)| &w.raster).ok_or("a band was not read")?;
                Ok((w, band))
            })
            .collect::<Result<_, String>>()?;
        let (raster, said) = stack(name, channels, &sources)?;
        notes.extend(said);
        Ok((raster, notes))
    }

    /// The bands as one raster of as many bands on the finest grid among them, by nearest cell, with
    /// the catalog's scale and offset applied. A cell with no value in any is no data in all.
    pub(super) fn stack(name: &str, channels: &[String], sources: &[(&Raster, &sb::Band)]) -> Result<(Raster, Vec<String>), String> {
        let (Some((first, _)), true) = (sources.first(), sources.len() >= 2) else { return Err("a stack needs at least two bands".into()) };
        if sources.iter().any(|(r, _)| r.crs != first.crs) {
            return Err("the bands are in different coordinate systems: they can't be put on one grid".into());
        }
        // A scale of 0 (or one that isn't a number) would turn every value of a band into its
        // offset: a catalog error to say, not a constant band to draw.
        for ((_, band), who) in sources.iter().zip(channels) {
            if band.unread {
                return Err(format!(
                    "{who}: the catalog gives its scale, offset or no-data as something that is not a number (a string, say), so its values can't be scaled"
                ));
            }
            if !band.scale.is_finite() || band.scale == 0.0 {
                return Err(format!("{who}: the catalog gives its values a scale of {}, so they can't be scaled", band.scale));
            }
        }
        let finest = sources
            .iter()
            .map(|(r, _)| *r)
            .min_by(|x, y| x.transform.pixel_width.abs().total_cmp(&y.transform.pixel_width.abs()))
            .ok_or("a stack needs at least two bands")?;
        let (w, h) = (finest.width as usize, finest.height as usize);
        let mut bands: Vec<Vec<f32>> = Vec::with_capacity(3);
        for (raster, band) in sources {
            let mut out = rustgis_analysis::rasterops::new_band(w, h, rustgis_analysis::rasterops::NODATA)?;
            let data = &raster.bands.get(band.index.saturating_sub(1) as usize).ok_or("a band is not in its file")?.data;
            let catalog_nodata = band.nodata.map(|v| v as f32);
            for row in 0..h {
                if row % 256 == 0 {
                    rustgis_data::progress::check().map_err(|e| e.to_string())?;
                }
                for col in 0..w {
                    let world = finest.transform.to_world(col as f64 + 0.5, row as f64 + 0.5);
                    let (sc, sr) = raster.transform.to_pixel(world);
                    let (sc, sr) = (sc.floor(), sr.floor());
                    if sc < 0.0 || sr < 0.0 || sc >= f64::from(raster.width) || sr >= f64::from(raster.height) {
                        continue;
                    }
                    let Some(&v) = data.get(sr as usize * raster.width as usize + sc as usize) else { continue };
                    if raster.is_valid(v) && catalog_nodata != Some(v) {
                        let value = (f64::from(v) * band.scale + band.offset) as f32;
                        if value.is_finite()
                            && let Some(o) = out.get_mut(row * w + col)
                        {
                            *o = value;
                        }
                    }
                }
            }
            bands.push(out);
        }
        // A cell with no value in one band is no data in all: a colour from two bands is no colour.
        let none = rustgis_analysis::rasterops::NODATA;
        let mut left = 0usize;
        for i in 0..w * h {
            if bands.iter().any(|b| b.get(i).is_none_or(|v| *v == none)) {
                left += 1;
                bands.iter_mut().for_each(|b| {
                    if let Some(v) = b.get_mut(i) {
                        *v = none;
                    }
                });
            }
        }
        // Not one cell with a value in every band: an empty image is no result, so it is an error.
        if w > 0 && h > 0 && left == w * h {
            return Err(format!(
                "no cell has a value in every band ({}): the item's data doesn't cover this part of the map, or its files are empty here",
                channels.join(", ")
            ));
        }
        let mut notes = Vec::new();
        // A band taken from a colour image (the item has no file of its own for it) is not in the
        // units of the bands that come from band files: the catalog's scale and offset are for those.
        for ((_, band), who) in sources.iter().zip(channels) {
            if band.in_file > 1 {
                notes.push(format!(
                    "{who} is band {} of {}, a file of {} bands, as the item has no file of its own for it: its values may not be in the units of the other bands (a colour image's are 8-bit)",
                    band.index, band.asset, band.in_file
                ));
            }
        }
        // Integer files the catalog gives no scale for are used as stored: not reflectance, which the
        // indices' constants and a colour mixed with scaled bands assume. One notice, naming them.
        let unscaled: Vec<String> = sources
            .iter()
            .zip(channels)
            .filter(|((r, b), _)| !b.scaled && (r.data_type.starts_with("UInt") || r.data_type.starts_with("Int")))
            .map(|((r, _), who)| format!("{who} ({})", r.data_type))
            .collect();
        if !unscaled.is_empty() {
            notes.push(format!(
                "{} have no scale in the catalog, so their integer values are used as stored, not as reflectance: an index, or a colour mixing them with scaled bands, will be off",
                unscaled.join(", ")
            ));
        }
        let size = finest.transform.pixel_width.abs();
        let resampled: Vec<&str> = sources
            .iter()
            .zip(channels)
            .filter(|((r, _), _)| (r.transform.pixel_width.abs() - size).abs() > size * 0.01)
            .map(|(_, c)| c.as_str())
            .collect();
        if !resampled.is_empty() {
            notes.push(format!(
                "{} read on a coarser grid than the finest of the bands, and put on it by nearest cell",
                resampled.join(", ")
            ));
        }
        if left > 0 {
            let share = left as f64 * 100.0 / (w * h).max(1) as f64;
            notes.push(format!("{left} cells ({share:.1}%) have no value in at least one band (no data, or outside a band's window) and are no data in every band"));
        }
        let mut raster = rustgis_analysis::rasterops::derive(finest, name, bands)?;
        for (b, c) in raster.bands.iter_mut().zip(channels) {
            b.name = c.clone();
        }
        Ok((raster, notes))
    }

    /// The made raster, kept as a GeoTIFF and drawn as red, green and blue, with what to say of it
    /// once it is added (by its name on the map) and what the command answers besides.
    fn made_saying(raster: Raster, notes: Vec<String>, extra: Value) -> Result<Made, String> {
        let bytes = rustgis_data::raster::geotiff::write_all(&raster).map_err(|e| e.to_string())?;
        let name = raster.name.clone();
        let mut layer = RasterLayer::from_bytes(raster, format!("{name}.tif"), bytes);
        layer.style.renderer = RasterRenderer::Rgb { red: 0, green: 1, blue: 2 };
        let then = noted(name.clone(), notes);
        Ok(super::super::workflows::made_layer(name, layer, Some(extra), Some(then)))
    }

    /// What to say of a made layer once it is added: each note, by the layer's name on the map.
    fn noted(name: String, notes: Vec<String>) -> Then {
        Box::new(move |s: &mut Session, v: &mut Value| {
            let id = v.get("layer").and_then(Value::as_u64).and_then(|i| u32::try_from(i).ok());
            let added = id.and_then(|id| s.map().layer(id)).map_or(name, |l| l.name.clone());
            for note in notes {
                s.notify(format!("{added}: {note}"), false);
            }
        })
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[expect(clippy::unwrap_used, reason = "a test module gated on more than `test`, which clippy's allow-unwrap-in-tests does not see")]
mod tests {
    use super::desktop::{auto_cell, index_look, stack};
    use crate::render::rustgis_tiles::stac::bands::Band;
    use rustgis_data::raster::{ColorHint, GeoTransform, Raster, crs::Crs};
    use serde_json::json;

    /// A raster of one band on a grid with these cells (`size` degrees), `w × h` from the corner.
    fn grid(w: u32, h: u32, size: f64, values: Vec<f32>) -> Raster {
        let t = GeoTransform { origin_x: 10.0, origin_y: 45.0, pixel_width: size, pixel_height: -size };
        Raster::new("b", w, h, vec![values], t, Crs::from_epsg(4326).unwrap(), Some(0.0), ColorHint::Gray, "UInt16").unwrap()
    }

    fn band(scale: f64, offset: f64) -> Band {
        Band {
            asset: "x".into(),
            href: "https://x.org/x.tif".into(),
            index: 1,
            in_file: 1,
            common: String::new(),
            scale,
            offset,
            nodata: None,
            scaled: true,
            unread: false,
        }
    }

    #[test]
    fn a_combination_given_with_red_green_and_blue_is_refused_not_dropped() {
        let asset = |name: &str| {
            json!({"href": format!("https://example.org/{name}.tif"), "type": "image/tiff; application=geotiff; profile=cloud-optimized",
                "eo:bands": [{"common_name": name}]})
        };
        let item = json!({"type": "Feature", "id": "scene", "bbox": [10.0, 45.0, 10.02, 45.02], "properties": {"datetime": "2024-06-01T00:00:00Z"},
            "assets": {"red": asset("red"), "green": asset("green"), "blue": asset("blue"), "nir": asset("nir")}});
        let s = crate::Session::new(crate::Project::default());
        let refused = |p: serde_json::Value| super::bands_work(&s, &p).err();
        let both = refused(
            json!({"item": item, "combination": "colorInfrared", "red": "nir", "green": "red", "blue": "green", "bbox": [10.0, 45.0, 10.02, 45.02]}),
        );
        assert!(both.is_some_and(|e| e.contains("not both")), "the pair is refused");
        // Either alone is fine: the three bands, or a combination.
        assert!(
            refused(json!({"item": item, "red": "nir", "green": "red", "blue": "green", "bbox": [10.0, 45.0, 10.02, 45.02]})).is_none()
        );
        assert!(refused(json!({"item": item, "combination": "colorInfrared", "bbox": [10.0, 45.0, 10.02, 45.02]})).is_none());
        // A blank combination is not one.
        assert!(
            refused(
                json!({"item": item, "combination": " ", "red": "nir", "green": "red", "blue": "green", "bbox": [10.0, 45.0, 10.02, 45.02]})
            )
            .is_none()
        );
    }

    #[test]
    fn a_band_the_catalog_gave_unreadable_numbers_is_refused_and_an_unscaled_integer_file_is_said() {
        let a = grid(2, 2, 0.0001, vec![1000.0; 4]);
        let names: Vec<String> = vec!["nir".into(), "red".into()];
        // A scale given as text was not read: its band is refused, not drawn unscaled.
        let mut text = band(1.0, 0.0);
        (text.scaled, text.unread) = (false, true);
        let err = stack("s", &names, &[(&a, &band(1.0, 0.0)), (&a, &text)]).unwrap_err();
        assert!(err.starts_with("red:") && err.contains("not a number"), "{err}");
        // No scale on an integer file ("UInt16" in the grid helper): said once, naming the band and its type.
        let mut plain = band(1.0, 0.0);
        plain.scaled = false;
        let (_, notes) = stack("s", &names, &[(&a, &band(1.0, 0.0)), (&a, &plain)]).unwrap();
        assert!(notes.iter().any(|n| n.starts_with("red (UInt16) have no scale in the catalog")), "{notes:?}");
        // A stated scale says nothing, and neither does a float file without one.
        let (_, none) = stack("s", &names, &[(&a, &band(1.0, 0.0)), (&a, &band(1.0, 0.0))]).unwrap();
        assert!(!none.iter().any(|n| n.contains("no scale")), "{none:?}");
        let mut float = grid(2, 2, 0.0001, vec![0.5; 4]);
        float.data_type = "Float32".into();
        let (_, none) = stack("s", &names, &[(&a, &band(1.0, 0.0)), (&float, &plain)]).unwrap();
        assert!(!none.iter().any(|n| n.contains("no scale")), "reflectance as stored needs no note: {none:?}");
    }

    #[test]
    fn an_index_is_the_indices_formula_over_the_stacked_bands() {
        use crate::render::rustgis_tiles::stac::bands::INDICES;
        // Near infrared 0.5 and red 0.1 (stored 5000 and 1000, scaled 0.0001): NDVI = 0.4 / 0.6.
        let nir = grid(2, 2, 0.0001, vec![5000.0; 4]);
        let red = grid(2, 2, 0.0001, vec![1000.0; 4]);
        let ix = INDICES.iter().find(|i| i.id == "ndvi").unwrap();
        let names: Vec<String> = ix.channels.iter().map(|c| (*c).to_string()).collect();
        assert_eq!(names, ["nir", "red"]);
        let (stacked, _) = stack("s", &names, &[(&nir, &band(0.0001, 0.0)), (&red, &band(0.0001, 0.0))]).unwrap();
        assert_eq!(stacked.bands.len(), 2, "as many bands as the index reads");
        let expr = crate::cmd::imagery::index_expression("ndvi", 1, 1, 2, 1, 1).unwrap();
        let v = rustgis_analysis::rasterops::band_math(&stacked, &expr).unwrap();
        assert!(v.iter().all(|x| (x - 0.6667).abs() < 1e-3), "{v:?}");
        // EVI reads three bands (nir, red, blue): 2.5 · (0.5 − 0.1) / (0.5 + 6 · 0.1 − 7.5 · 0.05 + 1).
        let blue = grid(2, 2, 0.0001, vec![500.0; 4]);
        let evi = INDICES.iter().find(|i| i.id == "evi").unwrap();
        let names: Vec<String> = evi.channels.iter().map(|c| (*c).to_string()).collect();
        assert_eq!(names, ["nir", "red", "blue"]);
        let (stacked, _) =
            stack("s", &names, &[(&nir, &band(0.0001, 0.0)), (&red, &band(0.0001, 0.0)), (&blue, &band(0.0001, 0.0))]).unwrap();
        let expr = crate::cmd::imagery::index_expression("evi", 3, 1, 2, 1, 1).unwrap();
        assert_eq!(expr, "2.5 * (b1 - b2) / (b1 + 6 * b2 - 7.5 * b3 + 1)");
        let v = rustgis_analysis::rasterops::band_math(&stacked, &expr).unwrap();
        assert!(v.iter().all(|x| (x - 0.5797).abs() < 1e-3), "EVI of 0.5, 0.1, 0.05: {v:?}");
        // One band is not a stack; an unknown index is refused.
        assert!(stack("s", &names[..1], &[(&nir, &band(1.0, 0.0))]).is_err());
        assert!(crate::cmd::imagery::index_expression("nope", 1, 1, 1, 1, 1).is_err());
    }

    fn names() -> [String; 3] {
        ["red".into(), "green".into(), "blue".into()]
    }

    #[test]
    fn three_bands_become_one_raster_on_the_finest_grid_with_their_scale() {
        // Red and green at 10 m cells (4 × 4); blue at twice the size (2 × 2).
        let red = grid(4, 4, 0.0001, vec![1000.0; 16]);
        let green = grid(4, 4, 0.0001, vec![2000.0; 16]);
        let blue = grid(2, 2, 0.0002, vec![3000.0; 4]);
        let (r, notes) =
            stack("s", &names(), &[(&red, &band(0.0001, 0.0)), (&green, &band(0.0001, 0.0)), (&blue, &band(0.0001, -0.1))]).unwrap();
        assert_eq!((r.width, r.height, r.bands.len()), (4, 4, 3));
        let values: Vec<f32> = r.bands.iter().map(|b| b.data[0]).collect();
        assert_eq!(values, [0.1, 0.2, 0.2], "reflectance: v · scale + offset");
        assert_eq!(r.bands.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(), ["red", "green", "blue"]);
        assert!(notes.iter().any(|n| n.contains("blue") && n.contains("coarser grid")), "{notes:?}");
        assert!(!notes.iter().any(|n| n.contains("no data")), "every cell has all three: {notes:?}");
    }

    #[test]
    fn a_cell_missing_from_one_band_is_no_data_in_all_and_said() {
        let mut gone = vec![1000.0; 16];
        gone[5] = 0.0; // the file's no data
        let ok = grid(4, 4, 0.0001, vec![1000.0; 16]);
        let holed = grid(4, 4, 0.0001, gone);
        let (r, notes) = stack("s", &names(), &[(&ok, &band(1.0, 0.0)), (&holed, &band(1.0, 0.0)), (&ok, &band(1.0, 0.0))]).unwrap();
        let none = rustgis_analysis::rasterops::NODATA;
        assert!(r.bands.iter().all(|b| b.data[5] == none), "no data in all three");
        assert!(r.bands.iter().all(|b| b.data[4] == 1000.0), "the cells beside it are kept");
        assert!(notes.iter().any(|n| n.contains("1 cells") && n.contains("no value in at least one band")), "{notes:?}");
    }

    #[test]
    fn a_window_that_does_not_reach_a_cell_leaves_it_no_data() {
        let wide = grid(4, 4, 0.0001, vec![1000.0; 16]);
        // Covers only the top-left 2 × 2 of the finest grid.
        let small = grid(2, 2, 0.0001, vec![1000.0; 4]);
        let (r, notes) = stack("s", &names(), &[(&wide, &band(1.0, 0.0)), (&wide, &band(1.0, 0.0)), (&small, &band(1.0, 0.0))]).unwrap();
        let none = rustgis_analysis::rasterops::NODATA;
        assert_eq!(r.bands[0].data.iter().filter(|v| **v == none).count(), 12);
        assert!(notes.iter().any(|n| n.contains("12 cells")), "{notes:?}");
    }

    #[test]
    fn every_index_is_drawn_in_colour_over_a_range_of_its_own() {
        use crate::render::rustgis_tiles::stac::bands::INDICES;
        use rustgis_map::Ramp;
        for ix in &INDICES {
            let (ramp, min, max) = index_look(ix.id);
            assert!(min < max && min.is_finite() && max.is_finite(), "{}: {min}..{max}", ix.id);
            assert_ne!(ramp, Ramp::Greys, "{} is not drawn in grey", ix.id);
        }
        // Vegetation in greens, water in blues.
        assert_eq!((index_look("ndvi").0, index_look("ndwi").0), (Ramp::Forest, Ramp::Lagoon));
    }

    #[test]
    fn bands_with_no_cell_in_common_are_an_error_not_an_empty_image() {
        // The first band has a value in the top-left cell only, the second in the bottom-right
        // cell only (0 is the files' no data): no cell has a value in both.
        let wide = grid(4, 4, 0.0001, vec![1000.0; 16]);
        let mut values = vec![0.0; 16];
        values[15] = 1000.0;
        let corner = grid(4, 4, 0.0001, values);
        let mut top = vec![0.0; 16];
        top[0] = 1000.0;
        let first = grid(4, 4, 0.0001, top);
        let err = stack("s", &["a".into(), "b".into()], &[(&first, &band(1.0, 0.0)), (&corner, &band(1.0, 0.0))]).unwrap_err();
        assert!(err.contains("no cell has a value in every band") && err.contains("a, b"), "{err}");
        // One cell in common is a result, said as before.
        let (r, notes) = stack("s", &["a".into(), "b".into()], &[(&wide, &band(1.0, 0.0)), (&corner, &band(1.0, 0.0))]).unwrap();
        assert_eq!(r.bands[0].data.iter().filter(|v| **v != rustgis_analysis::rasterops::NODATA).count(), 1);
        assert!(notes.iter().any(|n| n.contains("15 cells")), "{notes:?}");
    }

    #[test]
    fn a_band_taken_from_a_colour_image_is_said_and_a_band_file_is_not() {
        let a = grid(2, 2, 0.0001, vec![1000.0; 4]);
        let names: Vec<String> = vec!["nir".into(), "red".into()];
        let mut from_colour = band(1.0, 0.0);
        (from_colour.asset, from_colour.in_file) = ("visual".into(), 3);
        let (_, notes) = stack("s", &names, &[(&a, &band(1.0, 0.0)), (&a, &from_colour)]).unwrap();
        assert!(notes.iter().any(|n| n.starts_with("red is band 1 of visual, a file of 3 bands")), "{notes:?}");
        let (_, none) = stack("s", &names, &[(&a, &band(1.0, 0.0)), (&a, &band(1.0, 0.0))]).unwrap();
        assert!(!none.iter().any(|n| n.contains("a file of")), "files of their own say nothing: {none:?}");
    }

    #[test]
    fn a_band_with_a_scale_of_zero_is_refused_not_drawn_as_one_value() {
        let a = grid(2, 2, 0.0001, vec![1000.0; 4]);
        let names: Vec<String> = vec!["nir".into(), "red".into()];
        let err = stack("s", &names, &[(&a, &band(1.0, 0.0)), (&a, &band(0.0, 0.5))]).unwrap_err();
        assert!(err.starts_with("red:") && err.contains("scale of 0"), "{err}");
        let err = stack("s", &names, &[(&a, &band(f64::NAN, 0.0)), (&a, &band(1.0, 0.0))]).unwrap_err();
        assert!(err.starts_with("nir:") && err.contains("scale"), "{err}");
        // A negative scale is odd but is a scale.
        assert!(stack("s", &names, &[(&a, &band(-1.0, 0.0)), (&a, &band(1.0, 0.0))]).is_ok());
    }

    #[test]
    fn bands_in_different_coordinate_systems_are_refused() {
        let a = grid(2, 2, 0.0001, vec![1.0; 4]);
        let mut b = grid(2, 2, 0.0001, vec![1.0; 4]);
        b.crs = Crs::from_epsg(3857).unwrap();
        let err = stack("s", &names(), &[(&a, &band(1.0, 0.0)), (&b, &band(1.0, 0.0)), (&a, &band(1.0, 0.0))]).unwrap_err();
        assert!(err.contains("different coordinate systems"), "{err}");
    }

    #[test]
    fn the_cell_size_read_follows_the_box() {
        // A 10 km box is read at about 5 m cells (the full image of a 10 m scene); a 2° box coarser.
        let small = auto_cell([10.0, 45.0, 10.09, 45.09]);
        let large = auto_cell([10.0, 45.0, 12.0, 47.0]);
        assert!((4.0..6.0).contains(&small), "{small}");
        assert!(large > 100.0, "{large}");
        // A degenerate box doesn't give a zero cell.
        assert!(auto_cell([10.0, 45.0, 10.0, 45.0]) > 0.0);
    }
}
