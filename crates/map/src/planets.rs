//! Basemaps of the other celestial bodies (#223): Web Mercator raster tiles of the Moon, Mars,
//! Mercury, Venus, the Galilean moons, Titan, Pluto and Charon, from OpenPlanetaryMap and USGS
//! Astrogeology. The body switcher puts one under a map when it moves the map to its body.
//!
//! Where they come from:
//! - OpenPlanetaryMap's CARTO composites send CORS headers and are read directly.
//! - OpenPlanetaryMap's single-layer mosaics sit in S3 buckets that send no CORS headers, which the
//!   browser build can't read, so they go through the opengeos tile service (`tiles.geolibre.app`,
//!   shared with GeoLibre), which adds them and caches the tiles at the edge. They are TMS.
//! - USGS Astrogeology serves the other bodies as WMS in equirectangular lon/lat only; the same
//!   service warps them to Web Mercator tiles (and caches them, as the WMS draws each on request).

/// A planetary basemap: XYZ (or TMS) raster tiles of one body.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlanetBasemap {
    /// Its id, the tile layer's catalog id.
    pub id: &'static str,
    /// The [`rustgis_geom::Body`] it shows.
    pub body: &'static str,
    pub name: &'static str,
    pub url: &'static str,
    /// Rows counted from the south.
    pub tms: bool,
    /// The deepest zoom the source has (deeper ones are drawn from it).
    pub max_zoom: u8,
    pub attribution: &'static str,
}

/// A basemap the opengeos tile service serves, `$id` its dataset: `$service` `"opm"` passes an
/// OpenPlanetaryMap mosaic through with CORS headers, `"wms"` warps a USGS Astrogeology WMS layer
/// to Web Mercator.
macro_rules! served {
    ($id:literal, $body:literal, $name:literal, $service:literal, $tms:literal, $max:literal, $attr:literal) => {
        PlanetBasemap {
            id: concat!("planet-", $id),
            body: $body,
            name: $name,
            url: concat!("https://tiles.geolibre.app/", $service, "/", $id, "/{z}/{x}/{y}.png"),
            tms: $tms,
            max_zoom: $max,
            attribution: $attr,
        }
    };
}

/// Every planetary basemap, grouped by body; a body's first is the one the switcher picks.
pub const BASEMAPS: [PlanetBasemap; 20] = [
    // The Moon.
    served!("moon-hillshaded-albedo", "moon", "Hillshaded Albedo", "opm", true, 6, "NASA / LOLA / USGS · OpenPlanetaryMap"),
    PlanetBasemap {
        id: "planet-moon-basemap-v0-1",
        body: "moon",
        name: "OpenPlanetaryMap Basemap",
        url: "https://cartocdn-gusc.global.ssl.fastly.net/opmbuilder/api/v1/map/named/opm-moon-basemap-v0-1/all/{z}/{x}/{y}.png",
        tms: false,
        max_zoom: 6,
        attribution: "OpenPlanetaryMap",
    },
    // Mars.
    served!("mars-viking-mdim21", "mars", "Viking MDIM2.1", "opm", true, 7, "NASA / Viking / USGS · OpenPlanetaryMap"),
    PlanetBasemap {
        id: "planet-mars-basemap-v0-2",
        body: "mars",
        name: "OpenPlanetaryMap Basemap",
        url: "https://cartocdn-gusc.global.ssl.fastly.net/opmbuilder/api/v1/map/named/opm-mars-basemap-v0-2/all/{z}/{x}/{y}.png",
        tms: false,
        max_zoom: 6,
        attribution: "OpenPlanetaryMap",
    },
    served!("mars-mola-color-noshade", "mars", "MOLA Colour Elevation", "opm", true, 6, "NASA / MOLA · OpenPlanetaryMap"),
    served!("mars-mola-color", "mars", "MOLA Shaded Colour Elevation", "opm", true, 6, "NASA / MOLA · OpenPlanetaryMap"),
    served!("mars-mola-gray", "mars", "MOLA Shaded Grey Elevation", "opm", true, 9, "NASA / MOLA · OpenPlanetaryMap"),
    served!("mars-hillshade", "mars", "Hillshade", "opm", true, 6, "NASA / MOLA · OpenPlanetaryMap"),
    // The other bodies, by name.
    served!("callisto-galileo-voyager", "callisto", "Galileo / Voyager", "wms", false, 6, "NASA / JPL · USGS Astrogeology"),
    served!("charon-mosaic", "charon", "New Horizons Mosaic", "wms", false, 6, "NASA / JHU APL / SwRI · USGS Astrogeology"),
    served!("europa-galileo-voyager", "europa", "Galileo / Voyager", "wms", false, 6, "NASA / JPL · USGS Astrogeology"),
    served!("ganymede-galileo-voyager", "ganymede", "Galileo / Voyager", "wms", false, 6, "NASA / JPL · USGS Astrogeology"),
    served!("io-galileo-color", "io", "Galileo SSI Colour", "wms", false, 6, "NASA / JPL · USGS Astrogeology"),
    served!("mercury-messenger-color", "mercury", "MESSENGER Colour Mosaic", "wms", false, 7, "NASA / JHU APL / CIW · USGS Astrogeology"),
    served!("mercury-messenger", "mercury", "MESSENGER Basemap", "wms", false, 7, "NASA / JHU APL · USGS Astrogeology"),
    served!("pluto-mosaic", "pluto", "New Horizons Mosaic", "wms", false, 7, "NASA / JHU APL / SwRI · USGS Astrogeology"),
    served!("pluto-color", "pluto", "New Horizons Colour Shaded Relief", "wms", false, 7, "NASA / JHU APL / SwRI · USGS Astrogeology"),
    served!("titan-cassini", "titan", "Cassini ISS Mosaic", "wms", false, 6, "NASA / JPL / SSI · USGS Astrogeology"),
    served!("titan-hisar", "titan", "Cassini HiSAR Mosaic", "wms", false, 6, "NASA / JPL / USGS · USGS Astrogeology"),
    served!("venus-magellan-color", "venus", "Magellan C3-MDIR Colour", "wms", false, 6, "NASA / JPL · USGS Astrogeology"),
];

/// A body's basemaps (none for the Earth, whose are the ordinary ones).
pub fn basemaps_of(body: &str) -> impl Iterator<Item = &'static PlanetBasemap> + '_ {
    BASEMAPS.iter().filter(move |b| b.body == body)
}

/// The planetary basemap with this id (with or without its `planet-` prefix).
pub fn basemap(id: &str) -> Option<&'static PlanetBasemap> {
    let id = id.trim();
    BASEMAPS.iter().find(|b| b.id == id || b.id.strip_prefix("planet-") == Some(id))
}

/// Whether a tile layer's catalog id is a planetary basemap's.
pub fn is_planet_basemap(id: &str) -> bool {
    BASEMAPS.iter().any(|b| b.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERVICE: &str = "https://tiles.geolibre.app/";

    #[test]
    fn every_body_but_the_earth_has_basemaps() {
        for body in rustgis_geom::BODIES {
            assert_eq!(basemaps_of(body.id).next().is_some(), !body.is_earth(), "{}", body.id);
        }
        for (i, b) in BASEMAPS.iter().enumerate() {
            assert!(rustgis_geom::Body::find(b.body).is_some(), "{}", b.id);
            assert!(b.url.starts_with("https://") && b.url.contains("{z}/{x}/{y}"), "{}", b.url);
            assert!(BASEMAPS.iter().skip(i + 1).all(|o| o.id != b.id), "{}", b.id);
            // The mosaics passed through are TMS; the warped WMS layers and CARTO's are XYZ.
            assert_eq!(b.url.starts_with(&format!("{SERVICE}opm/")), b.tms, "{}", b.id);
        }
        assert_eq!(basemap("mars-hillshade").map(|b| b.body), Some("mars"));
        assert_eq!(basemaps_of("moon").next().map(|b| b.id), Some("planet-moon-hillshaded-albedo"));
        assert!(is_planet_basemap("planet-titan-cassini") && !is_planet_basemap("usgs-imagery"));
    }
}
