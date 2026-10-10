//! L2: the map model — layers with symbology and labels, the camera, colour ramps and basemaps.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod camera;
pub mod color;
pub mod globe;
pub mod raster;
pub mod symbology;

pub use camera::Camera;
pub use color::{Ramp, Rgba};
pub use globe::{Globe, Projection};
pub use raster::{RasterLayer, RasterRenderer, RasterStyle, Resampling, Stretch};
pub use symbology::{ClassMethod, Labeling, Marker, Renderer, Symbol};

use rustgis_data::{Dataset, Geometry, GeometryType};
use rustgis_geom::{Coord, Extent, lonlat_to_merc};
use serde::{Deserialize, Serialize};

/// The backdrop drawn under every layer. Each is drawn from the built-in world outline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Basemap {
    #[default]
    Paper,
    Midnight,
    Ocean,
    Atlas,
    Blank,
}

impl Basemap {
    pub const ALL: [Basemap; 5] = [Basemap::Paper, Basemap::Midnight, Basemap::Ocean, Basemap::Atlas, Basemap::Blank];
    pub fn label(self) -> &'static str {
        match self {
            Basemap::Paper => "Paper",
            Basemap::Midnight => "Midnight",
            Basemap::Ocean => "Ocean",
            Basemap::Atlas => "Atlas",
            Basemap::Blank => "Blank",
        }
    }
    pub fn code(self) -> &'static str {
        match self {
            Basemap::Paper => "paper",
            Basemap::Midnight => "midnight",
            Basemap::Ocean => "ocean",
            Basemap::Atlas => "atlas",
            Basemap::Blank => "blank",
        }
    }
    pub fn from_code(s: &str) -> Option<Basemap> {
        Basemap::ALL.into_iter().find(|b| b.code().eq_ignore_ascii_case(s.trim()) || b.label().eq_ignore_ascii_case(s.trim()))
    }
    /// (water, land, land outline, graticule) colours.
    pub fn palette(self) -> (Rgba, Rgba, Rgba, Rgba) {
        match self {
            Basemap::Paper => {
                (Rgba::rgb(0xDD, 0xE4, 0xEA), Rgba::rgb(0xF6, 0xF5, 0xF1), Rgba::rgb(0xC9, 0xC9, 0xC4), Rgba::new(0x9A, 0xA8, 0xB4, 60))
            }
            Basemap::Midnight => {
                (Rgba::rgb(0x1A, 0x1F, 0x26), Rgba::rgb(0x2E, 0x33, 0x3A), Rgba::rgb(0x47, 0x4D, 0x55), Rgba::new(0x80, 0x90, 0xA0, 40))
            }
            Basemap::Ocean => {
                (Rgba::rgb(0x9C, 0xC6, 0xE0), Rgba::rgb(0xE9, 0xE4, 0xD4), Rgba::rgb(0xB8, 0xB0, 0x98), Rgba::new(0xFF, 0xFF, 0xFF, 90))
            }
            Basemap::Atlas => {
                (Rgba::rgb(0xB5, 0xD5, 0xE6), Rgba::rgb(0xDC, 0xE8, 0xC8), Rgba::rgb(0xA8, 0xB4, 0x90), Rgba::new(0x60, 0x80, 0x90, 50))
            }
            Basemap::Blank => (Rgba::rgb(0xFF, 0xFF, 0xFF), Rgba::rgb(0xFF, 0xFF, 0xFF), Rgba::new(0, 0, 0, 0), Rgba::new(0, 0, 0, 0)),
        }
    }
}

/// A layer: a dataset drawn with a renderer and labels.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Layer {
    pub id: u32,
    pub name: String,
    pub visible: bool,
    pub dataset: Dataset,
    pub renderer: Renderer,
    pub labels: Labeling,
    /// 0 = opaque, 100 = invisible.
    pub transparency: f32,
    /// Drawn only when zoomed in beyond 1:`min_scale` (out-beyond limit), if set.
    #[serde(default)]
    pub min_scale: Option<f64>,
    /// Drawn only when zoomed out beyond 1:`max_scale` (in-beyond limit), if set.
    #[serde(default)]
    pub max_scale: Option<f64>,
    /// Definition query: only matching features draw.
    #[serde(default)]
    pub definition_query: Option<String>,
    /// Selected rows (indices into `dataset.features`).
    #[serde(default)]
    pub selection: Vec<usize>,
    /// Whether the Contents pane shows the legend classes.
    #[serde(default = "yes")]
    pub expanded: bool,
    #[serde(default = "yes")]
    pub selectable: bool,
    /// Set for raster layers (whose `dataset` is empty).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raster: Option<RasterLayer>,
    #[serde(skip)]
    projected: Vec<Option<Geometry>>,
    #[serde(skip)]
    projected_rev: u64,
    /// Bumped whenever geometry changes so caches rebuild.
    #[serde(default)]
    pub geometry_rev: u64,
}

fn yes() -> bool {
    true
}

impl Layer {
    pub fn new(id: u32, dataset: Dataset) -> Self {
        let renderer = Renderer::Single(Symbol::default_for(dataset.geometry_type, id));
        Layer {
            id,
            name: dataset.name.clone(),
            visible: true,
            labels: Labeling::default_for(&dataset),
            dataset,
            renderer,
            transparency: 0.0,
            min_scale: None,
            max_scale: None,
            definition_query: None,
            selection: Vec::new(),
            expanded: true,
            selectable: true,
            raster: None,
            projected: Vec::new(),
            projected_rev: u64::MAX,
            geometry_rev: 0,
        }
    }

    /// A raster layer.
    pub fn new_raster(id: u32, name: impl Into<String>, raster: RasterLayer) -> Self {
        let name = name.into();
        let mut l = Layer::new(id, Dataset::new(name.clone(), GeometryType::None, Vec::new()));
        l.selectable = false;
        l.labels.enabled = false;
        l.raster = Some(raster);
        l
    }

    pub fn is_raster(&self) -> bool {
        self.raster.is_some()
    }

    pub fn geometry_type(&self) -> GeometryType {
        self.dataset.geometry_type
    }

    /// Web Mercator copies of every geometry, rebuilt lazily when geometry changes.
    pub fn projected(&mut self) -> &[Option<Geometry>] {
        if self.projected_rev != self.geometry_rev || self.projected.len() != self.dataset.features.len() {
            self.projected = self.dataset.features.iter().map(|f| f.geometry.as_ref().map(|g| g.map_coords(&lonlat_to_merc))).collect();
            self.projected_rev = self.geometry_rev;
        }
        &self.projected
    }

    /// The Web Mercator copies from the last [`Layer::projected`] call (empty before one).
    pub fn projected_cache(&self) -> &[Option<Geometry>] {
        &self.projected
    }

    /// Marks geometry as changed.
    pub fn touch_geometry(&mut self) {
        self.geometry_rev = self.geometry_rev.wrapping_add(1);
    }

    /// Whether the layer draws at a scale denominator.
    pub fn in_scale_range(&self, scale: f64) -> bool {
        self.min_scale.is_none_or(|m| scale <= m) && self.max_scale.is_none_or(|m| scale >= m)
    }

    /// Rows passing the definition query (all rows when none or invalid).
    pub fn visible_rows(&self) -> Vec<bool> {
        let n = self.dataset.features.len();
        match self.definition_query.as_deref().filter(|q| !q.trim().is_empty()) {
            Some(q) => match rustgis_data::query::parse(q, &self.dataset) {
                Ok(e) => (0..n).map(|r| rustgis_data::query::eval(&e, &self.dataset, r)).collect(),
                Err(_) => vec![true; n],
            },
            None => vec![true; n],
        }
    }

    /// Extent in longitude/latitude.
    pub fn extent(&self) -> Extent {
        match &self.raster {
            Some(r) => r.data.as_ref().map(|d| d.extent()).unwrap_or(Extent::EMPTY),
            None => self.dataset.extent(),
        }
    }

    pub fn selection_extent(&self) -> Extent {
        self.selection
            .iter()
            .filter_map(|r| self.dataset.features.get(*r))
            .filter_map(|f| f.geometry.as_ref())
            .fold(Extent::EMPTY, |e, g| e.union(&g.extent()))
    }

    pub fn opacity(&self) -> f32 {
        (1.0 - self.transparency / 100.0).clamp(0.0, 1.0)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bookmark {
    pub name: String,
    pub camera: Camera,
}

/// A map: an ordered list of layers (index 0 draws on top), a basemap and a camera.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Map {
    pub name: String,
    pub layers: Vec<Layer>,
    pub basemap: Basemap,
    pub camera: Camera,
    #[serde(default)]
    pub bookmarks: Vec<Bookmark>,
    #[serde(default = "yes")]
    pub graticule: bool,
    /// Flat map or 3D globe.
    #[serde(default)]
    pub projection: Projection,
    #[serde(default)]
    next_layer_id: u32,
}

impl Map {
    pub fn new(name: impl Into<String>) -> Self {
        Map {
            name: name.into(),
            layers: Vec::new(),
            basemap: Basemap::Paper,
            camera: Camera::world(),
            bookmarks: Vec::new(),
            graticule: true,
            projection: Projection::WebMercator,
            next_layer_id: 1,
        }
    }

    fn next_id(&mut self) -> u32 {
        let id = self.next_layer_id.max(self.layers.iter().map(|l| l.id).max().unwrap_or(0) + 1);
        self.next_layer_id = id + 1;
        id
    }

    /// Inserts a layer on top of the layers of its kind: points above lines above polygons above
    /// rasters, as GIS apps insert them.
    fn insert_ranked(&mut self, layer: Layer) {
        let rank = |l: &Layer| match (l.is_raster(), l.geometry_type()) {
            (true, _) => 4,
            (_, GeometryType::Point) => 0,
            (_, GeometryType::Polyline) => 1,
            (_, GeometryType::Polygon) => 2,
            (_, GeometryType::None) => 3,
        };
        let r = rank(&layer);
        let pos = self.layers.iter().position(|l| rank(l) >= r).unwrap_or(self.layers.len());
        self.layers.insert(pos, layer);
    }

    /// Adds a dataset as a new layer; returns its id.
    pub fn add_dataset(&mut self, ds: Dataset) -> u32 {
        let id = self.next_id();
        self.insert_ranked(Layer::new(id, ds));
        id
    }

    /// Adds a raster layer; returns its id.
    pub fn add_raster(&mut self, name: impl Into<String>, raster: RasterLayer) -> u32 {
        let id = self.next_id();
        self.insert_ranked(Layer::new_raster(id, name, raster));
        id
    }

    pub fn layer(&self, id: u32) -> Option<&Layer> {
        self.layers.iter().find(|l| l.id == id)
    }
    pub fn layer_mut(&mut self, id: u32) -> Option<&mut Layer> {
        self.layers.iter_mut().find(|l| l.id == id)
    }
    pub fn layer_index(&self, id: u32) -> Option<usize> {
        self.layers.iter().position(|l| l.id == id)
    }
    pub fn layer_by_name(&self, name: &str) -> Option<&Layer> {
        self.layers.iter().find(|l| l.name.eq_ignore_ascii_case(name))
    }

    /// Union of all layers' extents (lon/lat).
    pub fn full_extent(&self) -> Extent {
        self.layers.iter().filter(|l| l.visible).fold(Extent::EMPTY, |e, l| e.union(&l.extent()))
    }

    pub fn selection_count(&self) -> usize {
        self.layers.iter().map(|l| l.selection.len()).sum()
    }

    pub fn selection_extent(&self) -> Extent {
        self.layers.iter().fold(Extent::EMPTY, |e, l| e.union(&l.selection_extent()))
    }

    pub fn clear_selection(&mut self) {
        for l in &mut self.layers {
            l.selection.clear();
        }
    }

    /// Topmost feature under a Web Mercator point: (layer id, row).
    pub fn hit(&mut self, p: Coord, tol: f64, scale: f64) -> Option<(u32, usize)> {
        for l in &mut self.layers {
            if !l.visible || !l.in_scale_range(scale) {
                continue;
            }
            let rows = l.visible_rows();
            let id = l.id;
            let proj = l.projected();
            // Last drawn is on top.
            for (row, g) in proj.iter().enumerate().rev() {
                if rows.get(row) == Some(&true)
                    && let Some(g) = g
                    && g.hit(p, tol)
                {
                    return Some((id, row));
                }
            }
        }
        None
    }

    /// Rows of each selectable, visible layer inside a Web Mercator rectangle.
    pub fn hits_in(&mut self, e: &Extent, scale: f64) -> Vec<(u32, Vec<usize>)> {
        let mut out = Vec::new();
        for l in &mut self.layers {
            if !l.visible || !l.selectable || !l.in_scale_range(scale) {
                continue;
            }
            let rows = l.visible_rows();
            let id = l.id;
            let hits: Vec<usize> = l
                .projected()
                .iter()
                .enumerate()
                .filter(|(r, g)| rows.get(*r) == Some(&true) && g.as_ref().is_some_and(|g| g.intersects_extent(e)))
                .map(|(r, _)| r)
                .collect();
            if !hits.is_empty() {
                out.push((id, hits));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustgis_data::{Field, FieldType, Value};

    fn points() -> Dataset {
        let mut d = Dataset::new("pts", GeometryType::Point, vec![Field::new("n", FieldType::Integer)]);
        d.push(Some(Geometry::Point(Coord::new(0.0, 0.0))), vec![Value::Int(1)]);
        d.push(Some(Geometry::Point(Coord::new(10.0, 10.0))), vec![Value::Int(2)]);
        d
    }

    #[test]
    fn layer_order_and_hit() {
        let mut m = Map::new("Map");
        let poly = Dataset::new("poly", GeometryType::Polygon, vec![]);
        m.add_dataset(poly);
        let id = m.add_dataset(points());
        assert_eq!(m.layers[0].id, id, "points go on top of polygons");
        let p = lonlat_to_merc(Coord::new(10.0, 10.0));
        assert_eq!(m.hit(p, 1000.0, 1e6), Some((id, 1)));
        let e = Extent::from_points(lonlat_to_merc(Coord::new(-1.0, -1.0)), lonlat_to_merc(Coord::new(1.0, 1.0)));
        assert_eq!(m.hits_in(&e, 1e6), vec![(id, vec![0])]);
        m.layers[0].definition_query = Some("n = 2".into());
        assert_eq!(m.hits_in(&e, 1e6), vec![]);
    }
}
