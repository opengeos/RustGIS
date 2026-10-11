//! L2: the map model — layers with symbology and labels, the camera, colour ramps and basemaps.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod camera;
pub mod color;
pub mod globe;
pub mod graphics;
pub mod projection;
pub mod raster;
pub mod symbology;
pub mod world;

pub use camera::Camera;
pub use color::{Ramp, Rgba};
pub use globe::{Globe, Projection};
pub use raster::{RasterLayer, RasterRenderer, RasterStyle, Resampling, Stretch};
pub use symbology::{ClassMethod, LabelPlacement, Labeling, Marker, Renderer, Symbol};

use rustgis_data::{Dataset, Geometry, GeometryType};
use rustgis_geom::{Coord, Extent};
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
    /// A graphics layer: each feature carries its own style (see [`graphics`]).
    #[serde(default)]
    pub graphics: bool,
    /// Set for raster layers (whose `dataset` is empty).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raster: Option<RasterLayer>,
    #[serde(skip)]
    projected: Vec<Option<Geometry>>,
    #[serde(skip)]
    projected_rev: u64,
    /// The projection the cache is in.
    #[serde(skip)]
    projected_for: String,
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
            graphics: false,
            raster: None,
            projected: Vec::new(),
            projected_rev: u64::MAX,
            projected_for: String::new(),
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

    /// A graphics layer.
    pub fn new_graphics(id: u32, name: &str) -> Self {
        let mut l = Layer::new(id, graphics::dataset(name));
        l.graphics = true;
        l.labels.enabled = false;
        l
    }

    /// The symbol a row draws with.
    pub fn symbol(&self, row: usize) -> Symbol {
        if self.graphics { graphics::symbol(&self.dataset, row) } else { self.renderer.symbol_for(&self.dataset, row) }
    }

    pub fn is_raster(&self) -> bool {
        self.raster.is_some()
    }

    pub fn geometry_type(&self) -> GeometryType {
        self.dataset.geometry_type
    }

    /// Copies of every geometry in a map projection's coordinates (`key` names the projection),
    /// rebuilt lazily when the geometry or the projection changes. Points a projection can't show
    /// become NaN, which the renderer skips.
    pub fn projected_in(&mut self, flat: &projection::Flat, key: &str) -> &[Option<Geometry>] {
        if self.projected_rev != self.geometry_rev || self.projected.len() != self.dataset.features.len() || self.projected_for != key {
            let f = |c: Coord| flat.forward_checked(c).unwrap_or(Coord::new(f64::NAN, f64::NAN));
            self.projected = self.dataset.features.iter().map(|ft| ft.geometry.as_ref().map(|g| g.map_coords(&f))).collect();
            self.projected_rev = self.geometry_rev;
            self.projected_for = key.to_string();
        }
        &self.projected
    }

    /// Web Mercator copies of every geometry (see [`Layer::projected_in`]).
    pub fn projected(&mut self) -> &[Option<Geometry>] {
        self.projected_in(&projection::Flat::Mercator, "mercator")
    }

    /// The Web Mercator copies from the last [`Layer::projected`] call, without refreshing them:
    /// call `projected` first (it needs `&mut self`) whenever the geometry may have changed.
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
    /// Which layers a click identifies.
    #[serde(default)]
    pub popup_mode: PopupMode,
    /// Map-wide label display switches.
    #[serde(default)]
    pub label_display: LabelDisplay,
    /// The features labelled when labels were locked, as (layer id, row): only they are labelled
    /// while the lock holds, wherever the map moves. Filled by the renderer.
    #[serde(skip)]
    pub label_lock: Option<Vec<(u32, usize)>>,
    #[serde(default)]
    next_layer_id: u32,
}

/// Which layers a click in the map identifies.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PopupMode {
    /// The topmost feature under the pointer.
    #[default]
    Topmost,
    /// Every visible layer's feature under the pointer.
    Visible,
    /// The topmost feature of a selectable layer.
    Selectable,
    /// Only the layer selected in the Contents pane.
    SelectedInContents,
    /// No pop-ups.
    None,
}

impl PopupMode {
    pub const ALL: [PopupMode; 5] =
        [PopupMode::Topmost, PopupMode::Visible, PopupMode::Selectable, PopupMode::SelectedInContents, PopupMode::None];
    pub fn code(self) -> &'static str {
        match self {
            PopupMode::Topmost => "topmost",
            PopupMode::Visible => "visible",
            PopupMode::Selectable => "selectable",
            PopupMode::SelectedInContents => "selectedInContents",
            PopupMode::None => "none",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            PopupMode::Topmost => "Topmost Layer",
            PopupMode::Visible => "Visible Layers",
            PopupMode::Selectable => "Selectable Layers",
            PopupMode::SelectedInContents => "Selected in Contents",
            PopupMode::None => "No Pop-ups",
        }
    }
    pub fn from_code(s: &str) -> Option<PopupMode> {
        PopupMode::ALL.into_iter().find(|m| m.code().eq_ignore_ascii_case(s.trim()))
    }
}

/// Map-wide label switches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelDisplay {
    /// No labels are drawn (faster navigation).
    #[serde(default)]
    pub paused: bool,
    /// The labelled features stay fixed as the map moves.
    #[serde(default)]
    pub locked: bool,
    /// Labels that didn't fit are drawn too, in red.
    #[serde(default)]
    pub show_unplaced: bool,
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
            popup_mode: PopupMode::Topmost,
            label_display: LabelDisplay::default(),
            label_lock: None,
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

    /// Adds an empty graphics layer on top; returns its id.
    pub fn add_graphics(&mut self, name: &str) -> u32 {
        let id = self.next_id();
        self.layers.insert(0, Layer::new_graphics(id, name));
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

    /// The flat projection of the camera's coordinates (Web Mercator for the globe).
    pub fn flat(&self) -> projection::Flat {
        projection::Flat::of(&self.projection)
    }

    /// Longitude/latitude to the camera's coordinates; `None` outside the projection's area of
    /// use or where it folds.
    pub fn to_map(&self, c: Coord) -> Option<Coord> {
        self.flat().forward_checked(c)
    }

    /// The camera's coordinates to longitude/latitude.
    pub fn to_lonlat(&self, m: Coord) -> Option<Coord> {
        self.flat().inverse(m)
    }

    /// The longitude/latitude at the centre of view.
    pub fn center_lonlat(&self) -> Coord {
        self.to_lonlat(self.camera.center).unwrap_or(Coord::new(0.0, 0.0))
    }

    /// Ground metres per logical pixel at the centre of view.
    pub fn ground_res(&self) -> f64 {
        self.ground_res_at(&self.camera)
    }

    /// Ground metres per logical pixel for another camera on this map (an export's view).
    pub fn ground_res_at(&self, camera: &Camera) -> f64 {
        camera.res * self.flat().ground_per_unit(camera.center)
    }

    /// Map scale denominator at the centre of view.
    pub fn scale(&self) -> f64 {
        self.ground_res() / rustgis_geom::PIXEL_M
    }

    /// Sets the resolution from a scale denominator.
    pub fn set_scale(&mut self, scale: f64) {
        if !scale.is_finite() || scale <= 0.0 {
            return;
        }
        let per = self.flat().ground_per_unit(self.camera.center).max(1e-9);
        self.camera.res = scale * rustgis_geom::PIXEL_M / per;
        self.camera.clamp();
    }

    /// Points the flat map at a longitude/latitude extent with a margin.
    pub fn fit_lonlat(&mut self, e: &Extent, w: f64, h: f64) {
        let flat = self.flat();
        if let projection::Flat::Mercator = flat {
            self.camera.fit_lonlat(e, w, h);
            return;
        }
        if e.is_empty() || w <= 1.0 || h <= 1.0 {
            return;
        }
        let min_pad = if e.width() > 0.0 || e.height() > 0.0 { 1e-6 } else { 0.02 };
        let m = flat.extent(&e.padded(0.06, min_pad));
        if m.is_empty() {
            return;
        }
        self.camera.center = m.center();
        self.camera.res = (m.width() / w).max(m.height() / h).max(camera::MIN_RES * 2.0);
        self.camera.clamp();
    }

    /// Switches projection, keeping the centre and the ground scale.
    /// Bookmarks move with it, so they still show the same place.
    pub fn set_projection(&mut self, p: Projection) {
        let old = self.flat();
        let ground_of = |c: &Camera| (old.inverse(c.center).unwrap_or(Coord::new(0.0, 0.0)), c.res * old.ground_per_unit(c.center));
        let camera = ground_of(&self.camera);
        let marks: Vec<_> = self.bookmarks.iter().map(|b| ground_of(&b.camera)).collect();
        self.projection = p;
        let flat = self.flat();
        let place = |cam: &mut Camera, (center, ground): (Coord, f64)| {
            // A centre the new projection can't show moves to its origin.
            cam.center = flat.forward_checked(center).or_else(|| flat.forward(Coord::new(0.0, 0.0))).unwrap_or(Coord::new(0.0, 0.0));
            cam.res = ground / flat.ground_per_unit(cam.center).max(1e-9);
            cam.clamp();
        };
        place(&mut self.camera, camera);
        for (b, m) in self.bookmarks.iter_mut().zip(marks) {
            place(&mut b.camera, m);
        }
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

    /// Topmost feature under a point in the camera's coordinates: (layer id, row).
    pub fn hit(&mut self, p: Coord, tol: f64, scale: f64) -> Option<(u32, usize)> {
        let (flat, key) = (self.flat(), self.projection.code());
        for l in &mut self.layers {
            if !l.visible || !l.in_scale_range(scale) {
                continue;
            }
            let rows = l.visible_rows();
            let id = l.id;
            let proj = l.projected_in(&flat, &key);
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

    /// The topmost feature under a point in every visible layer, top layer first.
    pub fn hits_at(&mut self, p: Coord, tol: f64, scale: f64) -> Vec<(u32, usize)> {
        let (flat, key) = (self.flat(), self.projection.code());
        let mut out = Vec::new();
        for l in &mut self.layers {
            if !l.visible || !l.in_scale_range(scale) {
                continue;
            }
            let rows = l.visible_rows();
            let id = l.id;
            let proj = l.projected_in(&flat, &key);
            if let Some(row) = proj
                .iter()
                .enumerate()
                .rev()
                .find_map(|(row, g)| (rows.get(row) == Some(&true) && g.as_ref().is_some_and(|g| g.hit(p, tol))).then_some(row))
            {
                out.push((id, row));
            }
        }
        out
    }

    /// Rows of each selectable, visible layer inside a rectangle in the camera's coordinates.
    pub fn hits_in(&mut self, e: &Extent, scale: f64) -> Vec<(u32, Vec<usize>)> {
        let (flat, key) = (self.flat(), self.projection.code());
        let mut out = Vec::new();
        for l in &mut self.layers {
            if !l.visible || !l.selectable || !l.in_scale_range(scale) {
                continue;
            }
            let rows = l.visible_rows();
            let id = l.id;
            let hits: Vec<usize> = l
                .projected_in(&flat, &key)
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
    use rustgis_geom::lonlat_to_merc;

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
