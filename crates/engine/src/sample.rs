//! The built-in sample project: a world map from Natural Earth (public domain) data.

use rustgis_data::io::read_geojson;
use rustgis_geom::{Coord, lonlat_to_merc};
use rustgis_map::{Bookmark, Camera, ClassMethod, Marker, Ramp, Renderer, Rgba, Symbol};

use crate::Project;

pub const COUNTRIES: &str = include_str!("../../../assets/data/countries.geojson");
pub const CITIES: &str = include_str!("../../../assets/data/cities.geojson");
pub const RIVERS: &str = include_str!("../../../assets/data/rivers.geojson");
pub const LAKES: &str = include_str!("../../../assets/data/lakes.geojson");

fn cam(lon: f64, lat: f64, res: f64) -> Camera {
    Camera { center: lonlat_to_merc(Coord::new(lon, lat)), res }
}

/// "World Atlas": countries by population, cities sized by population, rivers and lakes.
pub fn world_project() -> Project {
    let mut p = Project::new("World Atlas");
    let map = p.map_mut();
    map.name = "World".into();
    map.basemap = rustgis_map::Basemap::Ocean;
    let mut add = |name: &str, text: &str| read_geojson(text, name).ok().map(|(ds, _)| map.add_dataset(ds));
    let countries = add("Countries", COUNTRIES);
    let lakes = add("Lakes", LAKES);
    let rivers = add("Rivers", RIVERS);
    let cities = add("Cities", CITIES);
    if let Some(l) = countries.and_then(|id| map.layer_mut(id)) {
        let base = Symbol {
            fill: Rgba::rgb(200, 200, 200),
            stroke: Rgba::rgb(255, 255, 255),
            stroke_width: 0.6,
            size: 0.0,
            marker: Marker::Circle,
        };
        if let Ok(r) = Renderer::graduated(&l.dataset, "pop_est", 5, ClassMethod::NaturalBreaks, Ramp::Lagoon, base) {
            l.renderer = r;
            l.renderer.map_symbols(|s| {
                s.stroke = Rgba::rgb(255, 255, 255);
                s.stroke_width = 0.6;
            });
        }
        l.labels.enabled = true;
        l.labels.field = "name".into();
        l.labels.size = 8.5;
        l.labels.color = Rgba::rgb(0x33, 0x3A, 0x44);
    }
    if let Some(l) = lakes.and_then(|id| map.layer_mut(id)) {
        l.renderer = Renderer::Single(Symbol {
            fill: Rgba::rgb(0x9C, 0xC6, 0xE0),
            stroke: Rgba::rgb(0x6E, 0xA4, 0xC8),
            stroke_width: 0.5,
            size: 0.0,
            marker: Marker::Circle,
        });
    }
    if let Some(l) = rivers.and_then(|id| map.layer_mut(id)) {
        l.renderer = Renderer::Single(Symbol {
            fill: Rgba::rgb(0x4F, 0x8F, 0xC6),
            stroke: Rgba::rgb(0x4F, 0x8F, 0xC6),
            stroke_width: 1.2,
            size: 0.0,
            marker: Marker::Circle,
        });
    }
    if let Some(l) = cities.and_then(|id| map.layer_mut(id)) {
        let base = Symbol {
            fill: Rgba::new(0xE0, 0x6A, 0x3B, 220),
            stroke: Rgba::rgb(255, 255, 255),
            stroke_width: 0.8,
            size: 6.0,
            marker: Marker::Circle,
        };
        if let Ok(r) = Renderer::proportional(&l.dataset, "pop_max", base) {
            l.renderer = r;
            if let Renderer::Proportional { min_size, max_size, .. } = &mut l.renderer {
                *min_size = 3.5;
                *max_size = 18.0;
            }
        }
        l.labels.field = "name".into();
        l.labels.size = 8.0;
        l.labels.bold = true;
        l.labels.color = Rgba::rgb(0x5A, 0x2A, 0x14);
        l.max_scale = None;
    }
    map.bookmarks = vec![
        Bookmark { name: "World".into(), camera: cam(10.0, 15.0, 26_000.0) },
        Bookmark { name: "Europe".into(), camera: cam(12.0, 50.0, 5_200.0) },
        Bookmark { name: "Africa".into(), camera: cam(19.0, 2.0, 8_500.0) },
        Bookmark { name: "Americas".into(), camera: cam(-80.0, 12.0, 15_000.0) },
        Bookmark { name: "East Asia".into(), camera: cam(118.0, 32.0, 6_500.0) },
    ];
    map.camera = cam(10.0, 18.0, 26_000.0);
    p
}
