//! Which bands a STAC item offers, and which band combinations can be made from them.
//!
//! A *band* is one raster band of one GeoTIFF asset, found by its common name (`red`, `green`,
//! `nir`, `swir16`…) that catalogs give in `eo:bands` or STAC 1.1's `bands`, or, for an asset of
//! one band the catalog names none for, by the asset's key. Sentinel-2 and Landsat publish one
//! file per band, so a colour image is made from three files. Pure: no network, no files; the
//! item is hostile input (counts are bounded and nothing is indexed past its end).

use super::Item;

/// The most bands an item is taken to offer (its assets are already capped).
const MAX_BANDS: usize = 256;

/// One band of one asset's file.
#[derive(Clone, Debug, PartialEq)]
pub struct Band {
    /// The asset's key.
    pub asset: String,
    /// The asset's address.
    pub href: String,
    /// Its place in the file, from 1.
    pub index: u32,
    /// How many bands its file has, by the catalog (1 when it names none).
    pub in_file: u32,
    /// Its common name, lower case; empty when the catalog names none.
    pub common: String,
    /// A stored value `v` means `v · scale + offset` (1 and 0 when the catalog gives none).
    pub scale: f64,
    pub offset: f64,
    /// The stored value the catalog says is no data, when it names one.
    pub nodata: Option<f64>,
}

/// A wanted name and the names that can stand in for it, best first.
const ALIASES: &[(&str, &[&str])] = &[
    ("nir", &["nir", "nir08", "nir09"]),
    ("swir16", &["swir16", "swir1"]),
    ("swir22", &["swir22", "swir2"]),
    ("rededge", &["rededge", "rededge1", "rededge2", "rededge3"]),
    ("coastal", &["coastal", "aerosol"]),
    ("lwir", &["lwir", "lwir11", "lwir12"]),
];

/// A band combination: three bands drawn as red, green and blue.
#[derive(Debug, PartialEq)]
pub struct Combination {
    /// A name for commands.
    pub id: &'static str,
    /// What a menu shows.
    pub name: &'static str,
    /// The band names drawn as red, green and blue.
    pub channels: [&'static str; 3],
}

/// The combinations known, in the order they are offered.
pub const COMBINATIONS: [Combination; 7] = [
    Combination { id: "trueColor", name: "True color", channels: ["red", "green", "blue"] },
    Combination { id: "colorInfrared", name: "Color infrared", channels: ["nir", "red", "green"] },
    Combination { id: "shortwaveInfrared", name: "Shortwave infrared", channels: ["swir22", "nir", "red"] },
    Combination { id: "falseColorUrban", name: "False color urban", channels: ["swir22", "swir16", "red"] },
    Combination { id: "agriculture", name: "Agriculture", channels: ["swir16", "nir", "blue"] },
    Combination { id: "geology", name: "Geology", channels: ["swir22", "swir16", "blue"] },
    Combination { id: "bathymetric", name: "Bathymetric", channels: ["red", "green", "coastal"] },
];

/// A spectral index: a ratio of the bands it names, made as one band of reflectance numbers. The
/// formulas are the Indices section's (`raster.index`); this says which bands each one reads.
#[derive(Debug, PartialEq)]
pub struct Index {
    /// A name for commands.
    pub id: &'static str,
    /// What a menu shows.
    pub name: &'static str,
    /// The band names it reads (see [`find`]), in the order they are read.
    pub channels: &'static [&'static str],
}

/// The indices known, in the order they are offered.
pub const INDICES: [Index; 6] = [
    Index { id: "ndvi", name: "NDVI (vegetation)", channels: &["nir", "red"] },
    Index { id: "ndwi", name: "NDWI (water)", channels: &["green", "nir"] },
    Index { id: "savi", name: "SAVI (vegetation over soil)", channels: &["nir", "red"] },
    Index { id: "nbr", name: "NBR (burn ratio)", channels: &["nir", "swir22"] },
    Index { id: "ndbi", name: "NDBI (built-up)", channels: &["swir16", "nir"] },
    Index { id: "evi", name: "EVI (enhanced vegetation)", channels: &["nir", "red", "blue"] },
];

/// The indices the item can make: those whose bands it has, in the order of [`INDICES`].
pub fn available_indices(bands: &[Band]) -> Vec<&'static Index> {
    INDICES.iter().filter(|i| i.channels.iter().all(|c| find(bands, c).is_some())).collect()
}

/// Every band of the item's GeoTIFF assets, asset by asset (as the item lists them) and in file
/// order. An asset the catalog describes no bands for is one band.
pub fn bands(item: &Item) -> Vec<Band> {
    let mut out = Vec::new();
    for asset in item.assets.iter().filter(|a| a.is_geotiff()) {
        let count = asset.bands.len().max(1);
        for i in 0..count {
            if out.len() >= MAX_BANDS {
                return out;
            }
            // An asset that names no bands has its first band's numbers on the asset itself.
            let info = asset.bands.get(i);
            let (scale, offset, nodata) = match info {
                Some(b) => (b.scale, b.offset, b.nodata),
                None if i == 0 => (asset.scale, asset.offset, asset.nodata),
                None => (None, None, None),
            };
            out.push(Band {
                asset: asset.key.clone(),
                href: asset.href.clone(),
                index: u32::try_from(i + 1).unwrap_or(u32::MAX),
                in_file: u32::try_from(count).unwrap_or(u32::MAX),
                common: info.map(|b| b.common.clone()).unwrap_or_default(),
                scale: scale.unwrap_or(1.0),
                offset: offset.unwrap_or(0.0),
                nodata,
            });
        }
    }
    out
}

/// The band `wanted` names (`red`, `nir`…): the first of its stand-ins (see [`ALIASES`]) that some
/// band has as its common name, else that an asset of exactly one band has as its key. Of several
/// bands with the name, one in a file of its own is taken over one of a stack: Sentinel-2's
/// `visual` and `preview` are colour images whose bands are named red, green and blue too, and
/// they are not the band files. A key of an asset with several bands says nothing of which one is
/// meant, so it is not taken for one, nor is the key of an asset whose one band the catalog names
/// something else (an asset `red` of `nir` is not red).
pub fn find<'a>(bands: &'a [Band], wanted: &str) -> Option<&'a Band> {
    let wanted = wanted.trim().to_ascii_lowercase();
    let names: Vec<&str> = match ALIASES.iter().find(|(name, _)| *name == wanted) {
        Some((_, stand_ins)) => stand_ins.to_vec(),
        None => vec![wanted.as_str()],
    };
    names.iter().find_map(|name| {
        let named = || bands.iter().filter(|b| b.common == *name);
        named().find(|b| b.in_file == 1).or_else(|| named().next()).or_else(|| {
            let mut of_asset = bands.iter().filter(|b| b.asset.eq_ignore_ascii_case(name));
            match (of_asset.next(), of_asset.next()) {
                (Some(only), None) if only.common.is_empty() => Some(only),
                _ => None,
            }
        })
    })
}

/// A note for an error about a band not found, when the item describes more bands than are kept
/// (an asset's past [`super::MAX_BANDS`], or the item's past this list's cap): the band may be one
/// of those left out. None when nothing was cut.
pub fn cut_note(item: &Item) -> Option<String> {
    if item.assets.iter().any(|a| a.bands_cut) {
        return Some(format!("an asset describes more than {} bands and only the first {} are read", super::MAX_BANDS, super::MAX_BANDS));
    }
    let total: usize = item.assets.iter().filter(|a| a.is_geotiff()).map(|a| a.bands.len().max(1)).sum();
    (total > MAX_BANDS).then(|| format!("the item has more than {MAX_BANDS} bands and only the first {MAX_BANDS} are listed"))
}

/// The combinations the item can make, each with the bands found for its red, green and blue:
/// those whose three bands it has, in the order of [`COMBINATIONS`].
pub fn available(bands: &[Band]) -> Vec<(&'static Combination, [Band; 3])> {
    COMBINATIONS
        .iter()
        .filter_map(|c| {
            let [r, g, b] = c.channels;
            Some((c, [find(bands, r)?.clone(), find(bands, g)?.clone(), find(bands, b)?.clone()]))
        })
        .collect()
}

/// [`available`] without the bands found, for a menu that is drawn every frame: no band is cloned.
pub fn offered(bands: &[Band]) -> Vec<&'static Combination> {
    COMBINATIONS.iter().filter(|c| c.channels.iter().all(|channel| find(bands, channel).is_some())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stac::parse_items;
    use serde_json::json;

    /// An item with these assets (key, `eo:bands`/`raster:bands` JSON or null).
    fn item(assets: &[(&str, serde_json::Value)]) -> Item {
        let assets: serde_json::Map<String, serde_json::Value> = assets
            .iter()
            .map(|(key, extra)| {
                let mut a =
                    json!({"href": format!("https://x.org/{key}.tif"), "type": "image/tiff; application=geotiff; profile=cloud-optimized"});
                if let (Some(a), Some(e)) = (a.as_object_mut(), extra.as_object()) {
                    a.extend(e.clone());
                }
                (key.to_string(), a)
            })
            .collect();
        let body = json!({"type": "FeatureCollection", "features": [
            {"type": "Feature", "id": "i", "bbox": [0, 0, 1, 1], "properties": {"datetime": "2024-06-01T00:00:00Z"}, "assets": assets}
        ]})
        .to_string();
        parse_items(body.as_bytes(), "https://x.org/search").unwrap().0.remove(0)
    }

    fn named(common: &str) -> serde_json::Value {
        json!({"eo:bands": [{"common_name": common}], "raster:bands": [{"scale": 0.0001, "offset": -0.1, "nodata": 0}]})
    }

    /// A Sentinel-2 style item: a file a band, the names in `eo:bands`.
    fn sentinel() -> Item {
        item(&[
            ("blue", named("blue")),
            ("green", named("green")),
            ("red", named("red")),
            ("nir", named("nir")),
            ("nir08", named("nir08")),
            ("swir16", named("swir16")),
            ("swir22", named("swir22")),
            ("thumbnail", json!({"type": "image/png", "href": "https://x.org/thumbnail.png"})),
        ])
    }

    #[test]
    fn an_items_bands_come_from_its_geotiff_assets_with_their_numbers() {
        let b = bands(&sentinel());
        assert_eq!(b.len(), 7, "the thumbnail is not a band");
        let red = find(&b, "red").unwrap();
        assert_eq!((red.asset.as_str(), red.index, red.common.as_str()), ("red", 1, "red"));
        assert_eq!((red.scale, red.offset, red.nodata), (0.0001, -0.1, Some(0.0)));
    }

    #[test]
    fn a_name_is_found_by_its_stand_ins_best_first() {
        let b = bands(&sentinel());
        // Both `nir` and `nir08` are there: the wanted name's own is the best.
        assert_eq!(find(&b, "nir").unwrap().asset, "nir");
        // Landsat has only `nir08`: it stands in for `nir`.
        let landsat = bands(&item(&[("red", named("red")), ("nir08", named("nir08"))]));
        assert_eq!(find(&landsat, "nir").unwrap().asset, "nir08");
        assert_eq!(find(&landsat, " NIR ").unwrap().asset, "nir08", "case and spaces are not part of a name");
        let old = bands(&item(&[("swir1", named("swir1")), ("swir2", named("swir2"))]));
        assert_eq!((find(&old, "swir16").unwrap().asset.as_str(), find(&old, "swir22").unwrap().asset.as_str()), ("swir1", "swir2"));
        assert!(find(&b, "lwir").is_none());
    }

    /// Sentinel-2 items carry `visual` and `preview` colour images whose three bands are named red,
    /// green and blue; `preview` sorts before `red`, and it is a file of three, not the red band.
    #[test]
    fn a_band_in_a_file_of_its_own_is_taken_over_one_in_a_colour_image() {
        let colour = json!({"eo:bands": [{"common_name": "red"}, {"common_name": "green"}, {"common_name": "blue"}]});
        let b = bands(&item(&[("red", named("red")), ("preview", colour.clone()), ("visual", colour.clone())]));
        let red = find(&b, "red").unwrap();
        assert_eq!((red.asset.as_str(), red.index, red.in_file), ("red", 1, 1), "not preview's first band");
        // With no file of its own, the colour image's band is still found, by its place in it.
        let only = bands(&item(&[("visual", colour)]));
        let red = find(&only, "red").unwrap();
        assert_eq!((red.asset.as_str(), red.index, red.in_file), ("visual", 1, 3));
    }

    #[test]
    fn an_assets_key_is_not_its_band_when_the_catalog_names_the_band_something_else() {
        // An asset `red` whose one band is near infrared is not red; with no name it still is.
        let b = bands(&item(&[("red", named("nir")), ("green", json!({}))]));
        assert!(find(&b, "red").is_none(), "`red` holds the near infrared band");
        assert_eq!(find(&b, "nir").unwrap().asset, "red", "and it is found as what it is");
        assert_eq!(find(&b, "green").unwrap().asset, "green", "a band the catalog names none for goes by its key");
    }

    #[test]
    fn only_the_indices_an_item_can_make_are_offered() {
        let ids = |i: &Item| available_indices(&bands(i)).iter().map(|x| x.id).collect::<Vec<_>>();
        assert_eq!(ids(&sentinel()), ["ndvi", "ndwi", "savi", "nbr", "ndbi", "evi"]);
        // Visible light only: no near infrared, so no index.
        assert!(ids(&item(&[("red", named("red")), ("green", named("green")), ("blue", named("blue"))])).is_empty());
        // Near infrared and red: the vegetation indices, not the ones that need green, blue or SWIR.
        assert_eq!(ids(&item(&[("red", named("red")), ("nir", named("nir"))])), ["ndvi", "savi"]);
        // Every index reads bands the combinations' vocabulary knows.
        assert!(INDICES.iter().all(|i| i.channels.len() >= 2 && i.channels.iter().all(|c| !c.is_empty())));
    }

    #[test]
    fn an_asset_with_no_names_is_known_by_its_key_only_when_it_is_one_band() {
        // Named by key alone: one band each.
        let by_key = bands(&item(&[("red", json!({})), ("green", json!({})), ("blue", json!({}))]));
        assert_eq!(find(&by_key, "green").unwrap().asset, "green");
        // A file of several bands, none named: its key doesn't say which one is red.
        let stack = bands(&item(&[("red", json!({"raster:bands": [{}, {}, {}]}))]));
        assert_eq!(stack.len(), 3);
        assert!(find(&stack, "red").is_none());
        // Named inside the file: each is found by its own name and number.
        let named_stack = bands(&item(&[(
            "analytic",
            json!({"eo:bands": [{"common_name": "blue"}, {"common_name": "green"}, {"common_name": "red"}, {"common_name": "nir"}]}),
        )]));
        assert_eq!((find(&named_stack, "red").unwrap().index, find(&named_stack, "nir").unwrap().index), (3, 4));
    }

    #[test]
    fn only_the_combinations_an_item_can_make_are_offered() {
        let all = available(&bands(&sentinel()));
        let ids: Vec<&str> = all.iter().map(|(c, _)| c.id).collect();
        assert_eq!(
            ids,
            ["trueColor", "colorInfrared", "shortwaveInfrared", "falseColorUrban", "agriculture", "geology"],
            "no coastal band: no bathymetric"
        );
        let (_, [r, g, b]) = &all[1];
        assert_eq!((r.asset.as_str(), g.asset.as_str(), b.asset.as_str()), ("nir", "red", "green"), "colour infrared is nir, red, green");
        // Only visible light: true color and nothing else.
        let visible = available(&bands(&item(&[("red", named("red")), ("green", named("green")), ("blue", named("blue"))])));
        assert_eq!(visible.iter().map(|(c, _)| c.id).collect::<Vec<_>>(), ["trueColor"]);
        assert!(available(&bands(&item(&[("red", named("red"))]))).is_empty());
        assert!(available(&[]).is_empty());
        // `offered` is the same list without cloning the bands.
        for scene in [sentinel(), item(&[("red", named("red")), ("nir", named("nir"))]), item(&[])] {
            let all = bands(&scene);
            assert_eq!(
                offered(&all).iter().map(|c| c.id).collect::<Vec<_>>(),
                available(&all).iter().map(|(c, _)| c.id).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn an_item_passed_on_as_json_keeps_the_bands_it_needs() {
        // What a search answers, handed to a command: the names and the scaling survive.
        let original = sentinel();
        let again = crate::stac::item_from_json(&crate::stac::item_json(&original), None).unwrap();
        assert_eq!(bands(&again), bands(&original));
        assert_eq!(available(&bands(&again)).len(), 6);
        // A file of three named bands keeps each one's name and place.
        let stack = item(&[("analytic", json!({"eo:bands": [{"common_name": "blue"}, {"common_name": "green"}, {"common_name": "red"}]}))]);
        let again = crate::stac::item_from_json(&crate::stac::item_json(&stack), None).unwrap();
        assert_eq!(find(&bands(&again), "red").map(|b| b.index), Some(3));
        // An asset that names no band but has a scale keeps it, and one with none writes no `bands`.
        let scaled = item(&[("red", json!({"raster:bands": [{"scale": 0.5, "offset": 1.0}]})), ("green", json!({}))]);
        let again = crate::stac::item_from_json(&crate::stac::item_json(&scaled), None).unwrap();
        let (red, green) = (find(&bands(&again), "red").unwrap().clone(), find(&bands(&again), "green").unwrap().clone());
        assert_eq!((red.scale, red.offset), (0.5, 1.0));
        assert_eq!((green.scale, green.offset), (1.0, 0.0));
        let written = crate::stac::item_json(&scaled);
        assert!(written["assets"]["green"].get("bands").is_none(), "nothing to say of a band the catalog describes none of");
    }

    #[test]
    fn the_unified_bands_of_stac_1_1_are_read_like_eo_bands() {
        let unified = json!({"bands": [{"eo:common_name": "Red", "raster:scale": 0.5, "raster:offset": 2.0, "nodata": 255}]});
        let b = bands(&item(&[("data", unified)]));
        let red = find(&b, "red").unwrap();
        assert_eq!((red.scale, red.offset, red.nodata), (0.5, 2.0, Some(255.0)));
    }

    #[test]
    fn hostile_band_lists_are_bounded_and_odd_numbers_are_not_taken() {
        // More bands than are kept, and a no-data that is not a number.
        let many: Vec<serde_json::Value> = (0..500).map(|_| json!({"common_name": "red"})).collect();
        let b = bands(&item(&[("big", json!({"eo:bands": many}))]));
        assert_eq!(b.len(), 64, "one asset's bands are cut");
        let nan = bands(&item(&[("red", json!({"eo:bands": [{"common_name": "red"}], "raster:bands": [{"nodata": "nan", "scale": 0}]}))]));
        assert_eq!(nan[0].nodata, None, "\"nan\" is not a number");
        assert_eq!(nan[0].scale, 0.0, "a stated scale is kept as stated");
        // The cut is said: for the asset, and again after the item has been passed to a command.
        let big = item(&[("big", json!({"eo:bands": (0..500).map(|_| json!({})).collect::<Vec<_>>()}))]);
        assert!(cut_note(&big).is_some_and(|n| n.contains("more than 64 bands")), "{:?}", cut_note(&big));
        let again = crate::stac::item_from_json(&crate::stac::item_json(&big), None).unwrap();
        assert!(cut_note(&again).is_some(), "the cut survives the item being passed on");
        // An item with more bands than the list keeps (six files of 64) says so too; a whole one says nothing.
        let sixty_four = json!({"eo:bands": (0..64).map(|_| json!({})).collect::<Vec<_>>()});
        let wide = item(&[
            ("a", sixty_four.clone()),
            ("b", sixty_four.clone()),
            ("c", sixty_four.clone()),
            ("d", sixty_four.clone()),
            ("e", sixty_four.clone()),
            ("f", sixty_four),
        ]);
        assert_eq!(bands(&wide).len(), 256, "the list stops at its cap");
        assert!(cut_note(&wide).is_some_and(|n| n.contains("more than 256 bands")), "{:?}", cut_note(&wide));
        assert_eq!(cut_note(&sentinel()), None);
        // A name cut to a sane length, and nothing panics on junk.
        let junk = bands(&item(&[("x", json!({"eo:bands": [1, "a", null, {"common_name": 7}], "raster:bands": "no"}))]));
        assert_eq!(junk.len(), 4);
        assert!(find(&junk, "red").is_none());
    }
}
