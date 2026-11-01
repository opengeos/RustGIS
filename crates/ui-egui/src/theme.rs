//! Design tokens (light and dark), interface metrics, the theme file format and the egui style.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, Visuals};

use crate::i18n;

/// RustGIS brand colour (the Project tab and the logo) and its lighter tint.
pub const BRAND: Color32 = Color32::from_rgb(0xB8, 0x4A, 0x2C);
pub const BRAND_DARK: Color32 = Color32::from_rgb(0x96, 0x3A, 0x22);
/// Contextual tab group colour (Feature Layer, Labeling, Data).
pub const CONTEXT: Color32 = Color32::from_rgb(0x0E, 0x93, 0x84);

/// Why a theme file could not be read.
#[derive(Debug)]
pub enum Over {
    TooBig,
    Reason(String),
}

/// Reads a theme file without trusting its length: a pipe or a device cannot exhaust memory.
pub fn read_text(path: &std::path::Path) -> Result<String, Over> {
    use std::io::Read;
    // Only a plain file is opened: opening a pipe blocks until a writer arrives.
    let meta = std::fs::metadata(path).map_err(|e| Over::Reason(e.to_string()))?;
    if !meta.file_type().is_file() {
        return Err(Over::Reason(i18n::fmt("this is not a file", &[])));
    }
    let file = std::fs::File::open(path).map_err(|e| Over::Reason(e.to_string()))?;
    let mut bytes = Vec::new();
    file.take(MAX_THEME_BYTES + 1).read_to_end(&mut bytes).map_err(|e| Over::Reason(e.to_string()))?;
    // Measured in bytes: a cut inside a character is a file too big, not a file badly encoded.
    if bytes.len() as u64 > MAX_THEME_BYTES {
        return Err(Over::TooBig);
    }
    String::from_utf8(bytes).map_err(|e| Over::Reason(e.to_string()))
}

/// The refusal a file over the limit gets, the same from the folder and from the picker.
pub fn too_big(path: &std::path::Path) -> String {
    let limit = (MAX_THEME_BYTES / 1024).to_string();
    why(path, &i18n::fmt("a theme file is at most {limit} KB", &[("limit", &limit)]))
}

/// A path and the reason it failed, as the menu shows it.
fn why(path: &std::path::Path, reason: &str) -> String {
    i18n::fmt("{path}: {reason}", &[("path", &path.display().to_string()), ("reason", reason)])
}

/// The only schema this build reads.
pub const SCHEMA: u32 = 1;

/// A larger file is refused before it is read.
pub const MAX_THEME_BYTES: u64 = 64 * 1024;

/// One ribbon line fits; a longer name is cut to it.
const MAX_NAME: usize = 48;

/// Every colour a theme file must set. A theme that omits one is refused, and the error names it.
pub const TOKENS: [&str; 31] = [
    "header",
    "ribbon",
    "tab_strip",
    "canvas",
    "pane",
    "pane_header",
    "border",
    "border_strong",
    "text",
    "text_dim",
    "text_disabled",
    "icon",
    "hover",
    "pressed",
    "checked",
    "accent",
    "accent_text",
    "on_accent",
    "input",
    "input_border",
    "group_label",
    "status_bar",
    "selection",
    "menu",
    "shadow",
    "green",
    "red",
    "amber",
    "context",
    "context_tint",
    "control",
];

/// Every colour the interface uses; widgets never hard-code colours.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tokens {
    pub dark: bool,
    pub header: Color32,
    pub ribbon: Color32,
    pub tab_strip: Color32,
    pub canvas: Color32,
    pub pane: Color32,
    pub pane_header: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_disabled: Color32,
    pub icon: Color32,
    pub hover: Color32,
    pub pressed: Color32,
    pub checked: Color32,
    pub accent: Color32,
    pub accent_text: Color32,
    pub on_accent: Color32,
    pub input: Color32,
    pub input_border: Color32,
    pub group_label: Color32,
    pub status_bar: Color32,
    pub selection: Color32,
    pub menu: Color32,
    pub shadow: Color32,
    pub green: Color32,
    pub red: Color32,
    pub amber: Color32,
    pub context: Color32,
    pub context_tint: Color32,
    /// Slider rails and checkbox wells, which must stay visible on white.
    pub control: Color32,
}

impl Tokens {
    pub fn light() -> Self {
        Tokens {
            dark: false,
            header: Color32::from_rgb(0xEE, 0xF1, 0xF5),
            ribbon: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            tab_strip: Color32::from_rgb(0xEE, 0xF1, 0xF5),
            canvas: Color32::from_rgb(0xE4, 0xE8, 0xEE),
            pane: Color32::from_rgb(0xFB, 0xFC, 0xFD),
            pane_header: Color32::from_rgb(0xF3, 0xF5, 0xF8),
            border: Color32::from_rgb(0xDD, 0xE2, 0xE9),
            border_strong: Color32::from_rgb(0xC3, 0xCA, 0xD4),
            text: Color32::from_rgb(0x1F, 0x26, 0x30),
            text_dim: Color32::from_rgb(0x5E, 0x69, 0x77),
            text_disabled: Color32::from_rgb(0xA9, 0xB1, 0xBC),
            icon: Color32::from_rgb(0x3A, 0x45, 0x54),
            hover: Color32::from_rgb(0xE9, 0xF0, 0xFB),
            pressed: Color32::from_rgb(0xD4, 0xE2, 0xF8),
            checked: Color32::from_rgb(0xDC, 0xE8, 0xFA),
            accent: Color32::from_rgb(0x25, 0x63, 0xEB),
            accent_text: Color32::from_rgb(0x1D, 0x4E, 0xC4),
            on_accent: Color32::WHITE,
            input: Color32::WHITE,
            input_border: Color32::from_rgb(0xC6, 0xCD, 0xD7),
            group_label: Color32::from_rgb(0x70, 0x7A, 0x88),
            status_bar: Color32::from_rgb(0xF3, 0xF5, 0xF8),
            selection: Color32::from_rgb(0xCF, 0xE0, 0xFC),
            menu: Color32::WHITE,
            shadow: Color32::from_black_alpha(28),
            green: Color32::from_rgb(0x16, 0x9A, 0x5A),
            red: Color32::from_rgb(0xD1, 0x3C, 0x3C),
            amber: Color32::from_rgb(0xE5, 0x9E, 0x1A),
            context: CONTEXT,
            context_tint: Color32::from_rgb(0xE3, 0xF5, 0xF2),
            control: Color32::from_rgb(0xDD, 0xE3, 0xEA),
        }
    }

    pub fn dark() -> Self {
        Tokens {
            dark: true,
            header: Color32::from_rgb(0x17, 0x1B, 0x21),
            ribbon: Color32::from_rgb(0x24, 0x29, 0x31),
            tab_strip: Color32::from_rgb(0x17, 0x1B, 0x21),
            canvas: Color32::from_rgb(0x12, 0x15, 0x1A),
            pane: Color32::from_rgb(0x1E, 0x23, 0x2A),
            pane_header: Color32::from_rgb(0x24, 0x29, 0x31),
            border: Color32::from_rgb(0x34, 0x3B, 0x45),
            border_strong: Color32::from_rgb(0x48, 0x51, 0x5D),
            text: Color32::from_rgb(0xE6, 0xEA, 0xEF),
            text_dim: Color32::from_rgb(0x9A, 0xA4, 0xB1),
            text_disabled: Color32::from_rgb(0x5C, 0x65, 0x71),
            icon: Color32::from_rgb(0xC9, 0xD1, 0xDB),
            hover: Color32::from_rgb(0x2F, 0x37, 0x44),
            pressed: Color32::from_rgb(0x2A, 0x3D, 0x5E),
            checked: Color32::from_rgb(0x28, 0x3B, 0x5C),
            accent: Color32::from_rgb(0x5B, 0x8D, 0xF6),
            accent_text: Color32::from_rgb(0x8A, 0xAF, 0xFA),
            on_accent: Color32::WHITE,
            input: Color32::from_rgb(0x15, 0x19, 0x1F),
            input_border: Color32::from_rgb(0x44, 0x4D, 0x59),
            group_label: Color32::from_rgb(0x8C, 0x96, 0xA3),
            status_bar: Color32::from_rgb(0x1A, 0x1E, 0x24),
            selection: Color32::from_rgb(0x26, 0x40, 0x6E),
            menu: Color32::from_rgb(0x26, 0x2C, 0x34),
            shadow: Color32::from_black_alpha(110),
            green: Color32::from_rgb(0x3F, 0xC2, 0x82),
            red: Color32::from_rgb(0xF0, 0x6A, 0x6A),
            amber: Color32::from_rgb(0xF2, 0xB8, 0x48),
            context: Color32::from_rgb(0x2C, 0xC2, 0xAE),
            context_tint: Color32::from_rgb(0x1B, 0x33, 0x31),
            control: Color32::from_rgb(0x3A, 0x42, 0x4D),
        }
    }

    /// Falls back to the brightness when no theme was applied to the context.
    pub fn get(ctx: &egui::Context) -> Tokens {
        ctx.data(|d| d.get_temp::<Tokens>(tokens_id()))
            .unwrap_or_else(|| if ctx.global_style().visuals.dark_mode { Tokens::dark() } else { Tokens::light() })
    }
}

/// egui temp data key that lets every widget read the applied colours.
fn tokens_id() -> egui::Id {
    egui::Id::unique("rustgis.theme.tokens")
}

/// A theme file is a `.json` whatever the case of the extension.
fn is_theme_file(path: &std::path::Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("json"))
}

/// A file's `metrics` block: every field optional, and an unknown name is reported by name.
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct MetricsFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    item_spacing: Option<[f32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    button_padding: Option<[f32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    row_height: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    corner_radius: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    body: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    button_text: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    small: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    heading: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mono: Option<f32>,
}

impl MetricsFile {
    fn from(metrics: &Metrics) -> Self {
        MetricsFile {
            item_spacing: Some(metrics.item_spacing),
            button_padding: Some(metrics.button_padding),
            row_height: Some(metrics.row_height),
            corner_radius: Some(metrics.corner_radius),
            body: Some(metrics.body),
            button_text: Some(metrics.button_text),
            small: Some(metrics.small),
            heading: Some(metrics.heading),
            mono: Some(metrics.mono),
        }
    }

    /// Applies the sizes the file sets; the rest keep what the theme it extends gave them.
    fn apply_to(self, metrics: &mut Metrics) {
        if let Some(v) = self.item_spacing {
            metrics.item_spacing = v;
        }
        if let Some(v) = self.button_padding {
            metrics.button_padding = v;
        }
        if let Some(v) = self.row_height {
            metrics.row_height = v;
        }
        if let Some(v) = self.corner_radius {
            metrics.corner_radius = v;
        }
        if let Some(v) = self.body {
            metrics.body = v;
        }
        if let Some(v) = self.button_text {
            metrics.button_text = v;
        }
        if let Some(v) = self.small {
            metrics.small = v;
        }
        if let Some(v) = self.heading {
            metrics.heading = v;
        }
        if let Some(v) = self.mono {
            metrics.mono = v;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Metrics {
    pub item_spacing: [f32; 2],
    pub button_padding: [f32; 2],
    pub row_height: f32,
    pub corner_radius: u8,
    pub body: f32,
    pub button_text: f32,
    pub small: f32,
    pub heading: f32,
    pub mono: f32,
}

impl Metrics {
    pub fn standard() -> Self {
        Metrics {
            item_spacing: [6.0, 5.0],
            button_padding: [8.0, 3.0],
            row_height: 22.0,
            corner_radius: 5,
            body: 12.5,
            button_text: 12.5,
            small: 10.5,
            heading: 17.0,
            mono: 12.0,
        }
    }

    /// Held to what a window draws; a value that is not a number takes the standard.
    pub fn clamped(self) -> Self {
        let fit = |v: f32, lo: f32, hi: f32, otherwise: f32| if v.is_finite() { v.clamp(lo, hi) } else { otherwise };
        let std = Metrics::standard();
        Metrics {
            item_spacing: [
                fit(self.item_spacing[0], 0.0, 40.0, std.item_spacing[0]),
                fit(self.item_spacing[1], 0.0, 40.0, std.item_spacing[1]),
            ],
            button_padding: [
                fit(self.button_padding[0], 0.0, 40.0, std.button_padding[0]),
                fit(self.button_padding[1], 0.0, 40.0, std.button_padding[1]),
            ],
            row_height: fit(self.row_height, 8.0, 80.0, std.row_height),
            corner_radius: self.corner_radius.min(32),
            body: fit(self.body, 6.0, 40.0, std.body),
            button_text: fit(self.button_text, 6.0, 40.0, std.button_text),
            small: fit(self.small, 5.0, 32.0, std.small),
            heading: fit(self.heading, 8.0, 60.0, std.heading),
            mono: fit(self.mono, 5.0, 40.0, std.mono),
        }
    }
}

/// The interface scale a command accepts, held between 80 and 200 percent.
pub fn clamp_scale(scale: f32) -> f32 {
    if scale.is_finite() { scale.clamp(0.8, 2.0) } else { 1.0 }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    pub name: String,
    pub dark: bool,
    pub tokens: Tokens,
    pub metrics: Metrics,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    schema: u32,
    name: String,
    /// A theme that ships, whose colours and sizes this file starts from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    extends: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dark: Option<bool>,
    #[serde(default)]
    colours: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    metrics: Option<MetricsFile>,
}

impl Theme {
    /// `themes/*.json` beside the crate; an unreadable file falls back to the code tables.
    pub fn builtin() -> Vec<Theme> {
        [("Light", include_str!("../themes/light.json"), Tokens::light()), ("Dark", include_str!("../themes/dark.json"), Tokens::dark())]
            .into_iter()
            .map(|(name, text, fallback)| {
                Theme::parse(name, text).unwrap_or(Theme {
                    name: name.to_string(),
                    dark: fallback.dark,
                    tokens: fallback,
                    metrics: Metrics::standard(),
                })
            })
            .collect()
    }

    /// A refused file names the key at fault.
    pub fn parse(fallback_name: &str, text: &str) -> Result<Theme, String> {
        // Notepad saves UTF-8 with a byte-order mark, which serde refuses at the first byte.
        let text = text.trim_start_matches('\u{feff}');
        let raw: Raw = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let name = clean_name(if raw.name.trim().is_empty() { fallback_name } else { raw.name.trim() });
        if raw.schema != SCHEMA {
            return Err(i18n::fmt(
                "{name}: theme schema {found} is not {schema}",
                &[("name", name.as_str()), ("found", &raw.schema.to_string()), ("schema", &SCHEMA.to_string())],
            ));
        }
        let extends = raw.extends.is_some();
        let base = match raw.extends.as_deref() {
            None => None,
            Some(wanted) => match Theme::builtin().into_iter().find(|t| t.name == wanted) {
                Some(found) => Some(found),
                None => {
                    return Err(i18n::fmt(
                        "{name}: extends {wanted}, which is not a built-in theme",
                        &[("name", name.as_str()), ("wanted", wanted)],
                    ));
                }
            },
        };
        let unknown: Vec<&str> = raw.colours.keys().map(String::as_str).filter(|k| !TOKENS.contains(k)).collect();
        if !unknown.is_empty() {
            let names = unknown.join(", ");
            return Err(i18n::fmt("{name}: unknown {names}", &[("name", name.as_str()), ("names", &names)]));
        }
        let missing: Vec<&str> = match extends {
            true => Vec::new(),
            false => TOKENS.iter().copied().filter(|t| !raw.colours.contains_key(*t)).collect(),
        };
        if !missing.is_empty() {
            let names = missing.join(", ");
            return Err(i18n::fmt("{name}: missing {names}", &[("name", name.as_str()), ("names", &names)]));
        }
        let fallback = base.unwrap_or_else(|| Theme {
            name: name.clone(),
            dark: raw.dark.unwrap_or(false),
            tokens: if raw.dark.unwrap_or(false) { Tokens::dark() } else { Tokens::light() },
            metrics: Metrics::standard(),
        });
        let mut tokens = fallback.tokens;
        for (key, value) in &raw.colours {
            let colour = parse_colour(value)
                .map_err(|e| i18n::fmt("{name}: {key} {reason}", &[("name", name.as_str()), ("key", key.as_str()), ("reason", &e)]))?;
            if let Some(slot) = slot(&mut tokens, key) {
                *slot = colour;
            }
        }
        let dark = raw.dark.unwrap_or(fallback.dark);
        // apply() draws from this flag, so a base theme cannot decide the brightness.
        tokens.dark = dark;
        let mut metrics = fallback.metrics;
        if let Some(file) = raw.metrics {
            file.apply_to(&mut metrics);
        }
        let metrics = metrics.clamped();
        Ok(Theme { name, dark, tokens, metrics })
    }

    pub fn to_json(&self) -> Result<String, String> {
        let colours: BTreeMap<String, String> = TOKENS.iter().map(|name| ((*name).to_string(), hex(colour(self, name)))).collect();
        let raw = Raw {
            schema: SCHEMA,
            name: self.name.clone(),
            extends: None,
            dark: Some(self.dark),
            colours,
            metrics: Some(MetricsFile::from(&self.metrics)),
        };
        serde_json::to_string_pretty(&raw)
            .map_err(|e| i18n::fmt("{name}: {reason}", &[("name", self.name.as_str()), ("reason", &e.to_string())]))
    }

    /// Built-in themes plus the user folder's readable files; failures are returned apart.
    pub fn available() -> (Vec<Theme>, Vec<String>) {
        Self::available_in(Self::user_dir().as_deref())
    }

    /// The same, from a named folder; a missing folder is not an error.
    pub fn available_in(dir: Option<&std::path::Path>) -> (Vec<Theme>, Vec<String>) {
        let mut themes = Theme::builtin();
        let mut errors = Vec::new();
        let Some(dir) = dir else {
            return (themes, errors);
        };
        let Ok(read) = std::fs::read_dir(dir) else {
            return (themes, errors);
        };
        let mut files: Vec<PathBuf> = read.flatten().map(|e| e.path()).filter(|p| is_theme_file(p)).collect();
        files.sort();
        for path in files {
            let stem = path.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned());
            match read_text(&path) {
                Ok(text) => match Theme::parse(&stem, &text) {
                    // A user file named as a built-in replaces it, so a name picks one theme.
                    Ok(theme) => match themes.iter_mut().find(|t| t.name == theme.name) {
                        Some(replaced) => *replaced = theme,
                        None => themes.push(theme),
                    },
                    Err(e) => errors.push(why(&path, &e)),
                },
                Err(Over::TooBig) => errors.push(too_big(&path)),
                Err(Over::Reason(reason)) => errors.push(why(&path, &reason)),
            }
        }
        (themes, errors)
    }

    /// Hash of each file's name, size and mtime, from metadata alone: no file is read.
    pub fn folder_signature(dir: Option<&std::path::Path>) -> u64 {
        use std::hash::{Hash, Hasher};
        let Some(dir) = dir else { return 0 };
        let Ok(read) = std::fs::read_dir(dir) else { return 0 };
        let mut stamps: Vec<(String, u64, u128)> = read
            .flatten()
            .filter(|e| is_theme_file(&e.path()))
            .filter_map(|e| {
                let path = e.path();
                let meta = e.metadata().ok()?;
                // Metadata carries a std clock; this runs only where a themes folder exists.
                let saved = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis();
                Some((path.file_name()?.to_string_lossy().into_owned(), meta.len(), saved))
            })
            .collect();
        stamps.sort();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        stamps.hash(&mut hasher);
        hasher.finish()
    }

    /// `<config>/themes`, where a user theme is read from.
    pub fn user_dir() -> Option<PathBuf> {
        rustgis_engine::data::dirs::config_dir().map(|d| d.join("themes"))
    }

    /// The file that holds this theme: its own name, or the next free one when another theme took it.
    fn save_path(&self, dir: &std::path::Path) -> Result<PathBuf, String> {
        let stem = stem_of(&self.name);
        for at in 1..=99 {
            let name = if at == 1 { format!("{stem}.json") } else { format!("{stem}-{at}.json") };
            let path = dir.join(name);
            match read_text(&path) {
                Ok(text) => {
                    if Theme::parse(&stem, &text).is_ok_and(|found| found.name == self.name) {
                        return Ok(path);
                    }
                }
                // A file of another theme, or one too big to be read, is left alone.
                Err(Over::TooBig) => {}
                Err(Over::Reason(_)) => return Ok(path),
            }
        }
        Err(why(dir, &i18n::fmt("no free file name for {name}", &[("name", &self.name)])))
    }

    /// The file name of a theme, with each non-alphanumeric character as `-`.
    pub fn save_user(&self) -> Result<PathBuf, String> {
        let dir = Self::user_dir().ok_or_else(|| i18n::fmt("No settings folder to keep a theme in.", &[]))?;
        std::fs::create_dir_all(&dir).map_err(|e| why(&dir, &e.to_string()))?;
        let path = self.save_path(&dir)?;
        let text = self.to_json()?;
        backup_before_replace(&path, &text)?;
        std::fs::write(&path, text).map_err(|e| why(&path, &e.to_string()))?;
        Ok(path)
    }
}

/// The mutable slot for one token name, so the file writer, the reader and the tests share a list.
fn slot<'a>(t: &'a mut Tokens, name: &str) -> Option<&'a mut Color32> {
    Some(match name {
        "header" => &mut t.header,
        "ribbon" => &mut t.ribbon,
        "tab_strip" => &mut t.tab_strip,
        "canvas" => &mut t.canvas,
        "pane" => &mut t.pane,
        "pane_header" => &mut t.pane_header,
        "border" => &mut t.border,
        "border_strong" => &mut t.border_strong,
        "text" => &mut t.text,
        "text_dim" => &mut t.text_dim,
        "text_disabled" => &mut t.text_disabled,
        "icon" => &mut t.icon,
        "hover" => &mut t.hover,
        "pressed" => &mut t.pressed,
        "checked" => &mut t.checked,
        "accent" => &mut t.accent,
        "accent_text" => &mut t.accent_text,
        "on_accent" => &mut t.on_accent,
        "input" => &mut t.input,
        "input_border" => &mut t.input_border,
        "group_label" => &mut t.group_label,
        "status_bar" => &mut t.status_bar,
        "selection" => &mut t.selection,
        "menu" => &mut t.menu,
        "shadow" => &mut t.shadow,
        "green" => &mut t.green,
        "red" => &mut t.red,
        "amber" => &mut t.amber,
        "context" => &mut t.context,
        "context_tint" => &mut t.context_tint,
        "control" => &mut t.control,
        _ => return None,
    })
}

pub fn colour(theme: &Theme, name: &str) -> Color32 {
    let mut tokens = theme.tokens;
    slot(&mut tokens, name).map_or(Color32::TRANSPARENT, |c| *c)
}

/// `#RRGGBB`, or `#RRGGBBAA` where the alpha matters, as in a shadow.
fn hex(c: Color32) -> String {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    if a == 255 { format!("#{r:02X}{g:02X}{b:02X}") } else { format!("#{r:02X}{g:02X}{b:02X}{a:02X}") }
}

/// WCAG contrast ratio between two colours, 1.0 to 21.0.
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let lum = |c: Color32| {
        let [r, g, b, _] = c.to_srgba_unmultiplied();
        let channel = |v: u8| {
            let v = f32::from(v) / 255.0;
            if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
    };
    let (a, b) = (lum(a), lum(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// A name as the menu shows it: one line, no control characters, and at most [`MAX_NAME`] long.
fn clean_name(name: &str) -> String {
    let kept: String = name.trim().chars().filter(|c| !c.is_control()).take(MAX_NAME).collect();
    let kept = kept.trim();
    if kept.is_empty() { "Theme".to_string() } else { kept.to_string() }
}

/// Keeps the file it replaces beside it as `<name>.json.bak`: a folder copy may hold edits.
fn backup_before_replace(path: &std::path::Path, text: &str) -> Result<(), String> {
    // The same bound as any other read: a file over the limit is left alone, not loaded whole.
    let Ok(old) = read_text(path) else { return Ok(()) };
    if old == text {
        return Ok(());
    }
    let kept = path.with_extension("json.bak");
    std::fs::write(&kept, old).map_err(|e| why(&kept, &e.to_string()))
}

/// A value as a message shows it: long enough to recognise, short enough to stay one line.
fn clip(text: &str) -> String {
    let mut out: String = text.chars().take(32).collect();
    if out.chars().count() < text.chars().count() {
        out.push('…');
    }
    out
}

/// The stem of a theme's file name; a name of marks and symbols still gets one.
fn stem_of(name: &str) -> String {
    let dashed: String = name.chars().map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let stem = dashed.trim_matches('-').to_string();
    if stem.is_empty() { "theme".to_string() } else { stem }
}

/// A non-ASCII colour is refused before the byte cut, which would land inside a character.
fn parse_colour(text: &str) -> Result<Color32, String> {
    let shown = clip(text);
    let digits = text.strip_prefix('#').ok_or_else(|| i18n::fmt("is not a #RRGGBB colour: {text}", &[("text", &shown)]))?;
    let bad = || i18n::fmt("is not a #RRGGBB or #RRGGBBAA colour: {text}", &[("text", &shown)]);
    if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(bad());
    }
    let pair = |at: usize| {
        let pair = digits.get(at..at + 2).ok_or_else(bad)?;
        u8::from_str_radix(pair, 16).map_err(|_| bad())
    };
    match digits.len() {
        6 => Ok(Color32::from_rgb(pair(0)?, pair(2)?, pair(4)?)),
        8 => Ok(Color32::from_rgba_unmultiplied(pair(0)?, pair(2)?, pair(4)?, pair(6)?)),
        _ => Err(bad()),
    }
}

pub fn regular(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

/// Installs the interface fonts: an installed system sans-serif when available, then `extra`
/// faces (name, bytes, collection index) as fallbacks for scripts it lacks, such as CJK.
pub fn install_fonts(
    ctx: &egui::Context,
    extra: &[(String, Arc<FontData>, Option<rustgis_engine::render::fonts::ScriptFont>)],
    current: Option<rustgis_engine::render::fonts::ScriptFont>,
) {
    let f = rustgis_engine::render::fonts::ui_fonts();
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert("ui".into(), Arc::new(FontData::from_owned(f.regular.clone())));
    // Heavier where the face is variable (macOS's system font is one file for every weight, #591),
    // as exports draw it; a face with no weight axis ignores it.
    let weight = egui::epaint::text::VariationCoords::new([(b"wght", rustgis_engine::render::fonts::SEMIBOLD_WEIGHT)]);
    let semibold = FontData::from_owned(f.semibold.clone()).tweak(egui::FontTweak { coords: weight, ..Default::default() });
    fonts.font_data.insert("ui-semibold".into(), Arc::new(semibold));
    if let Some(prop) = fonts.families.get_mut(&FontFamily::Proportional) {
        prop.insert(0, "ui".into());
        // egui's Hack face has the Georgian letters that Ubuntu Light and Noto Sans lack.
        prop.push("Hack".into());
    }
    let mut semi = vec!["ui-semibold".to_string(), "ui".to_string()];
    if let Some(prop) = fonts.families.get(&FontFamily::Proportional) {
        semi.extend(prop.iter().skip(1).cloned());
    }
    fonts.families.insert(FontFamily::Name("semibold".into()), semi);
    for (name, data, region) in extra {
        fonts.font_data.insert(name.clone(), Arc::clone(data));
        let primary = region.is_some() && *region == current;
        // Arabic script goes ahead of the interface face: egui shapes each run of one face on its
        // own, so a Persian line reads in order only when this face draws its Latin words too.
        let first = primary && *region == Some(rustgis_engine::render::fonts::ScriptFont::Arabic);
        for fam in [FontFamily::Proportional, FontFamily::Name("semibold".into())] {
            if let Some(v) = fonts.families.get_mut(&fam) {
                if first {
                    v.insert(0, name.clone());
                } else if primary {
                    // Right after the interface face, so its regional glyph shapes win.
                    v.insert(1.min(v.len()), name.clone());
                } else {
                    v.push(name.clone());
                }
            }
        }
        // Monospace text (the status bar's bearing and pitch, code boxes) falls back to them too,
        // after Hack, so translated or CJK text in it doesn't draw as boxes.
        if let Some(v) = fonts.families.get_mut(&FontFamily::Monospace) {
            v.push(name.clone());
        }
    }
    ctx.set_fonts(fonts);
}

pub fn apply(ctx: &egui::Context, t: &Tokens, m: &Metrics) {
    let mut v = if t.dark { Visuals::dark() } else { Visuals::light() };
    let corner = CornerRadius::same(m.corner_radius);
    v.panel_fill = t.pane;
    v.window_fill = t.menu;
    v.extreme_bg_color = t.input;
    v.faint_bg_color = t.pane_header;
    v.window_stroke = Stroke::new(1.0, t.border_strong);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.selection.bg_fill = t.selection;
    v.selection.stroke = Stroke::new(1.0, t.accent);
    v.hyperlink_color = t.accent_text;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, t.border);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, t.text);
    v.widgets.inactive.bg_fill = t.control;
    v.slider_trailing_fill = true;
    v.widgets.inactive.weak_bg_fill = t.input;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, t.input_border);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, t.text);
    v.widgets.inactive.corner_radius = corner;
    v.widgets.hovered.bg_fill = t.hover;
    v.widgets.hovered.weak_bg_fill = t.hover;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, t.border_strong);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, t.text);
    v.widgets.hovered.corner_radius = corner;
    v.widgets.active.bg_fill = t.pressed;
    v.widgets.active.weak_bg_fill = t.pressed;
    v.widgets.active.fg_stroke = Stroke::new(1.0, t.text);
    v.widgets.active.corner_radius = corner;
    v.widgets.open.bg_fill = t.pressed;
    v.widgets.open.weak_bg_fill = t.pressed;
    v.popup_shadow = egui::Shadow { offset: [0, 6], blur: 18, spread: 0, color: t.shadow };
    v.window_shadow = egui::Shadow { offset: [0, 10], blur: 28, spread: 0, color: t.shadow };
    ctx.data_mut(|d| d.insert_temp(tokens_id(), *t));
    // Pin egui's theme to ours: the app decides when to follow the system (`dark_follows_system`).
    ctx.set_theme(if t.dark { egui::Theme::Dark } else { egui::Theme::Light });
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(m.item_spacing[0], m.item_spacing[1]);
        s.spacing.button_padding = egui::vec2(m.button_padding[0], m.button_padding[1]);
        s.spacing.interact_size.y = m.row_height;
        s.text_styles.insert(egui::TextStyle::Body, regular(m.body));
        s.text_styles.insert(egui::TextStyle::Button, regular(m.button_text));
        s.text_styles.insert(egui::TextStyle::Small, regular(m.small));
        s.text_styles.insert(egui::TextStyle::Heading, semibold(m.heading));
        s.text_styles.insert(egui::TextStyle::Monospace, FontId::monospace(m.mono));
    });
}

/// egui colour from a map colour.
pub fn c32(c: rustgis_engine::map::Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), c.a())
}

pub fn rgba(c: Color32) -> rustgis_engine::map::Rgba {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    rustgis_engine::map::Rgba::new(r, g, b, a)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn light() -> Theme {
        Theme::builtin().into_iter().find(|t| !t.dark).expect("a light theme ships")
    }

    #[test]
    fn the_built_in_files_match_the_code_tables() {
        let builtin = Theme::builtin();
        for (name, want) in [("Light", Tokens::light()), ("Dark", Tokens::dark())] {
            let theme = builtin.iter().find(|t| t.name == name).expect("the theme ships");
            assert_eq!(theme.dark, want.dark, "{name} brightness");
            for token in TOKENS {
                let mut copy = want;
                assert_eq!(colour(theme, token), *slot(&mut copy, token).expect("every token has a slot"), "{name} token {token}");
            }
            assert_eq!(theme.metrics, Metrics::standard(), "{name} metrics");
        }
    }

    #[test]
    fn every_built_in_theme_round_trips_and_sets_every_token() {
        for theme in Theme::builtin() {
            assert_eq!(Theme::parse(&theme.name, &theme.to_json().expect("json")).ok().as_ref(), Some(&theme));
            for token in TOKENS {
                assert!(colour(&theme, token).a() > 0, "{} leaves {token} invisible", theme.name);
            }
        }
    }

    #[test]
    fn every_built_in_theme_reads_its_text_on_its_surfaces() {
        for theme in Theme::builtin() {
            let ratio = contrast(theme.tokens.text, theme.tokens.pane);
            assert!(ratio >= 7.0, "{}: text on a pane is {ratio:.1}:1, below the AAA floor of 7:1", theme.name);
            let ratio = contrast(theme.tokens.text, theme.tokens.ribbon);
            assert!(ratio >= 7.0, "{}: text on the ribbon is {ratio:.1}:1", theme.name);
        }
    }

    #[test]
    fn a_file_saved_with_a_byte_order_mark_loads() {
        let text = format!("\u{feff}{}", light().to_json().expect("json"));
        assert_eq!(Theme::parse("Light", &text).expect("it loads").name, "Light");
    }

    #[test]
    fn a_file_that_extends_a_built_in_sets_only_what_it_changes() {
        let text = r##"{"schema": 1, "name": "Mine", "extends": "Dark", "colours": {"accent": "#F2A65A"}}"##;
        let theme = Theme::parse("mine", text).expect("it parses");
        assert_eq!(theme.name, "Mine");
        assert!(theme.dark, "it takes the brightness of the theme it extends");
        assert_eq!(theme.tokens.accent, Color32::from_rgb(0xF2, 0xA6, 0x5A));
        assert_eq!(theme.tokens.pane, Tokens::dark().pane, "the rest comes from Dark");
        assert_eq!(theme.metrics, Metrics::standard(), "sizes too");
    }

    #[test]
    fn extends_names_a_theme_that_ships() {
        let text = r#"{"schema": 1, "name": "Mine", "extends": "Gone"}"#;
        let err = Theme::parse("mine", text).expect_err("no theme called Gone ships");
        assert!(err.contains("extends Gone"), "{err}");
    }

    #[test]
    fn the_folder_signature_follows_a_saved_file() {
        let dir = theme_folder("rustgis-theme-signature-test", &[("mine.json", "{}")]);
        let before = Theme::folder_signature(Some(&dir));
        std::fs::write(dir.join("mine.json"), "{\"schema\": 1}").expect("the file");
        assert_ne!(Theme::folder_signature(Some(&dir)), before, "a saved file is noticed");
        assert_eq!(Theme::folder_signature(None), 0, "no folder never changes");
    }

    #[test]
    fn a_file_sets_the_sizes_it_names_and_keeps_the_rest() {
        let mut json: serde_json::Value = serde_json::from_str(&light().to_json().expect("json")).expect("json");
        json["extends"] = serde_json::json!("Light");
        json["colours"] = serde_json::json!({});
        json["metrics"] = serde_json::json!({ "body": 15.0 });
        let theme = Theme::parse("Mine", &json.to_string()).expect("a file may set one size");
        assert_eq!(theme.metrics.body, 15.0, "the size the file names");
        assert_eq!(theme.metrics.row_height, light().metrics.row_height, "a size it leaves keeps Light's");
    }

    #[test]
    fn a_size_that_does_not_exist_is_named() {
        let mut json: serde_json::Value = serde_json::from_str(&light().to_json().expect("json")).expect("json");
        json["metrics"] = serde_json::json!({ "bdy": 14.0 });
        let err = Theme::parse("Mine", &json.to_string()).expect_err("there is no size called bdy");
        assert!(err.contains("bdy"), "{err}");
    }

    #[test]
    fn a_colour_must_be_hex_digits() {
        let mut json: serde_json::Value = serde_json::from_str(&light().to_json().expect("json")).expect("json");
        json["colours"]["accent"] = serde_json::json!("#+F+F+F");
        let err = Theme::parse("Mine", &json.to_string()).expect_err("a plus sign is not a hex digit");
        assert!(err.to_lowercase().contains("accent"), "{err}");
    }

    #[test]
    fn a_missing_token_is_named() {
        let mut json: serde_json::Value = serde_json::from_str(&light().to_json().expect("json")).expect("json");
        json["colours"].as_object_mut().expect("colours").remove("accent");
        let err = Theme::parse("Mine", &json.to_string()).expect_err("a missing token is refused");
        assert!(err.contains("missing accent"), "{err}");
    }

    #[test]
    fn an_unknown_token_is_named() {
        let mut json: serde_json::Value = serde_json::from_str(&light().to_json().expect("json")).expect("json");
        json["colours"].as_object_mut().expect("colours").insert("acccent".into(), "#FF0000".into());
        let err = Theme::parse("Mine", &json.to_string()).expect_err("an unknown token is refused");
        assert!(err.contains("unknown acccent"), "{err}");
    }

    #[test]
    fn a_bad_colour_names_the_token() {
        let mut json: serde_json::Value = serde_json::from_str(&light().to_json().expect("json")).expect("json");
        json["colours"]["accent"] = "blue".into();
        let err = Theme::parse("Mine", &json.to_string()).expect_err("a bad colour is refused");
        assert!(err.contains("accent is not a #RRGGBB"), "{err}");
    }

    #[test]
    fn a_newer_schema_is_refused_by_number() {
        let mut json: serde_json::Value = serde_json::from_str(&light().to_json().expect("json")).expect("json");
        json["schema"] = 9.into();
        let err = Theme::parse("Mine", &json.to_string()).expect_err("another schema is refused");
        assert!(err.contains("schema 9 is not 1"), "{err}");
    }

    #[test]
    fn a_file_round_trips_with_its_alpha() {
        let theme = light();
        let back = Theme::parse("light", &theme.to_json().expect("json")).expect("its own file parses");
        assert_eq!(back, theme);
        assert_eq!(hex(theme.tokens.shadow), "#0000001C", "the shadow keeps its alpha");
    }

    #[test]
    fn a_nameless_file_takes_its_file_name() {
        let mut json: serde_json::Value = serde_json::from_str(&light().to_json().expect("json")).expect("json");
        json["name"] = "  ".into();
        let theme = Theme::parse("my-theme", &json.to_string()).expect("it parses");
        assert_eq!(theme.name, "my-theme");
    }

    #[test]
    fn every_token_name_has_its_own_slot() {
        let mut tokens = Tokens::light();
        for (i, name) in TOKENS.iter().enumerate() {
            *slot(&mut tokens, name).expect("a slot") = Color32::from_rgb(i as u8, 0, 0);
        }
        for (i, name) in TOKENS.iter().enumerate() {
            let mut copy = tokens;
            assert_eq!(slot(&mut copy, name).copied(), Some(Color32::from_rgb(i as u8, 0, 0)), "{name}");
        }
    }

    fn theme_folder(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("the folder");
        for (file, text) in files {
            std::fs::write(dir.join(file), text).expect("the file");
        }
        dir
    }

    #[test]
    fn a_theme_in_the_user_folder_is_offered() {
        let mine = light().to_json().expect("json").replace("\"Light\"", "\"Mine\"");
        let dir = theme_folder("rustgis-theme-folder-test", &[("mine.json", &mine)]);
        let (found, errors) = Theme::available_in(Some(&dir));
        assert!(errors.is_empty(), "{errors:?}");
        assert!(found.iter().any(|t| t.name == "Mine"), "the folder theme is offered");
        assert!(found.iter().any(|t| t.name == "Light"), "the built-in themes stay");
    }

    #[test]
    fn a_broken_file_is_reported_and_the_others_load() {
        let mine = light().to_json().expect("json").replace("\"Light\"", "\"Mine\"");
        let dir = theme_folder("rustgis-theme-broken-test", &[("mine.json", &mine), ("half.json", "{\"schema\": 1}")]);
        let (found, errors) = Theme::available_in(Some(&dir));
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("half.json"), "{errors:?}");
        assert!(found.iter().any(|t| t.name == "Mine"), "the good file still loads");
    }

    #[test]
    fn a_user_theme_replaces_the_built_in_of_the_same_name() {
        let mut json: serde_json::Value = serde_json::from_str(&light().to_json().expect("json")).expect("json");
        json["colours"]["accent"] = "#123456".into();
        let dir = theme_folder("rustgis-theme-replace-test", &[("light.json", &json.to_string())]);
        let (found, errors) = Theme::available_in(Some(&dir));
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(found.iter().filter(|t| t.name == "Light").count(), 1, "one Light is offered");
        let light = found.iter().find(|t| t.name == "Light").expect("Light is offered");
        assert_eq!(light.tokens.accent, Color32::from_rgb(0x12, 0x34, 0x56), "the user file wins");
    }

    #[test]
    fn apply_puts_the_theme_on_the_context() {
        let ctx = egui::Context::default();
        let mut theme = light();
        theme.tokens.pane = Color32::from_rgb(0x0A, 0x0B, 0x0C);
        theme.tokens.control = Color32::from_rgb(0x11, 0x12, 0x13);
        theme.tokens.accent = Color32::from_rgb(0x21, 0x22, 0x23);
        apply(&ctx, &theme.tokens, &theme.metrics);
        let style = ctx.global_style();
        assert_eq!(style.visuals.panel_fill, theme.tokens.pane);
        assert_eq!(style.visuals.widgets.inactive.bg_fill, theme.tokens.control);
        assert_eq!(style.spacing.item_spacing, egui::vec2(6.0, 5.0));
        assert_eq!(style.spacing.interact_size.y, 22.0);
        assert_eq!(style.text_styles[&egui::TextStyle::Body], regular(12.5));
        assert_eq!(style.text_styles[&egui::TextStyle::Heading], semibold(17.0));
        assert_eq!(Tokens::get(&ctx), theme.tokens, "widgets read the applied colours back");
    }

    #[test]
    fn the_brightness_of_the_file_is_the_brightness_drawn() {
        let text = r#"{"schema": 1, "name": "Mine", "extends": "Light", "dark": true}"#;
        let theme = Theme::parse("Mine", text).expect("a file may extend Light and be dark");
        assert!(theme.dark, "the file reports dark");
        assert!(theme.tokens.dark, "and apply() draws the dark visuals, not the base theme's");
    }

    #[test]
    fn a_dark_file_on_a_light_base_draws_dark() {
        let text = r##"{"schema": 1, "name": "Nox", "extends": "Light", "dark": true, "colours": {"accent": "#7C3AED"}}"##;
        let theme = Theme::parse("Nox", text).expect("a file may extend Light and be dark");
        let ctx = egui::Context::default();
        apply(&ctx, &theme.tokens, &theme.metrics);
        assert!(theme.dark && theme.tokens.dark, "the theme reports dark");
        assert!(ctx.global_style().visuals.dark_mode, "so egui draws the parts the file does not colour in dark mode");
        assert_eq!(theme.tokens.pane, Theme::builtin()[0].tokens.pane, "and the colours it extends stay");
    }

    #[test]
    fn a_name_of_symbols_still_gets_a_file_name() {
        assert_eq!(stem_of("★★★"), "theme", "a name with no letters or digits still saves");
        assert_eq!(stem_of("Cobalt Blue!"), "cobalt-blue", "letters and digits are kept, the rest as -");
    }

    #[test]
    fn a_replaced_theme_file_keeps_the_copy_it_replaced() {
        let mut json: serde_json::Value = serde_json::from_str(&light().to_json().expect("json")).expect("json");
        json["name"] = "Mine".into();
        json["colours"]["accent"] = "#123456".into();
        let edited = json.to_string();
        json["colours"]["accent"] = "#654321".into();
        let picked = json.to_string();
        let dir = theme_folder("rustgis-theme-backup-test", &[("mine.json", &edited)]);
        let path = dir.join("mine.json");
        backup_before_replace(&path, &picked).expect("the backup");
        assert_eq!(std::fs::read_to_string(dir.join("mine.json.bak")).expect("the backup file"), edited);
        assert_eq!(std::fs::read_to_string(&path).expect("the file"), edited, "the file is not touched yet");
        // The folder loads one Mine, and the backup is not a theme.
        std::fs::write(&path, &picked).expect("the file");
        let (found, errors) = Theme::available_in(Some(&dir));
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(found.iter().filter(|t| t.name == "Mine").count(), 1, "one Mine is offered");
        assert!(!found.iter().any(|t| t.name.contains("bak")), "the backup is not a theme");
        // Writing the same text again leaves the backup alone.
        backup_before_replace(&path, &picked).expect("no backup needed");
        assert_eq!(std::fs::read_to_string(dir.join("mine.json.bak")).expect("the backup file"), edited);
    }

    #[test]
    fn two_names_that_share_a_stem_keep_two_files() {
        let dir = theme_folder("rustgis-theme-stem-test", &[]);
        let mut first = light();
        first.name = "Cobalt Blue".to_string();
        let first_at = first.save_path(&dir).expect("a free name");
        std::fs::write(&first_at, first.to_json().expect("json")).expect("the file");
        let mut second = light();
        second.name = "Cobalt-Blue".to_string();
        let second_at = second.save_path(&dir).expect("another free name");
        std::fs::write(&second_at, second.to_json().expect("json")).expect("the file");
        assert_ne!(first_at, second_at, "two themes that share a stem keep two files");
        assert_eq!(first.save_path(&dir).expect("its own file"), first_at, "a theme finds the file that holds it");
        let (found, errors) = Theme::available_in(Some(&dir));
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(found.iter().filter(|t| t.name.starts_with("Cobalt")).count(), 2, "both load");
    }

    #[test]
    fn a_theme_file_with_an_upper_case_extension_is_read() {
        let mine = light().to_json().expect("json").replace("\"Light\"", "\"Mine\"");
        let dir = theme_folder("rustgis-theme-upper-test", &[("MINE.JSON", &mine)]);
        let (found, errors) = Theme::available_in(Some(&dir));
        assert!(errors.is_empty(), "{errors:?}");
        assert!(found.iter().any(|t| t.name == "Mine"), "the file is read whatever the case of its extension");
    }

    #[test]
    fn only_a_plain_file_is_opened() {
        let dir = theme_folder("rustgis-theme-dir-test", &[]);
        match read_text(&dir) {
            Err(Over::Reason(why)) => assert!(why.contains("not a file"), "{why}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_bad_value_is_echoed_short() {
        assert_eq!(clip("abc"), "abc", "a short value is kept as it is");
        assert!(clip(&"x".repeat(500)).chars().count() <= 33, "a long value is cut to one line");
    }

    #[test]
    fn a_file_over_the_limit_reports_the_limit_not_the_encoding() {
        let dir = theme_folder("rustgis-theme-multibyte-test", &[]);
        let path = dir.join("wide.json");
        // Two bytes each, so the cut lands inside a character.
        std::fs::write(&path, "ą".repeat(MAX_THEME_BYTES as usize / 2 + 1)).expect("the file");
        assert!(matches!(read_text(&path), Err(Over::TooBig)), "the size is reported, not the encoding");
    }

    #[test]
    fn a_theme_file_over_the_limit_is_not_read() {
        let dir = theme_folder("rustgis-theme-over-limit", &[]);
        let path = dir.join("big.json");
        std::fs::write(&path, "x".repeat(MAX_THEME_BYTES as usize + 1)).expect("the file");
        assert!(matches!(read_text(&path), Err(Over::TooBig)), "a file over the limit is refused as it is read");
    }

    #[test]
    fn metrics_are_held_within_what_the_interface_draws() {
        let mut json: serde_json::Value = serde_json::from_str(&light().to_json().expect("json")).expect("json");
        json["metrics"]["body"] = serde_json::json!(1e9);
        json["metrics"]["small"] = serde_json::json!(-5.0);
        json["metrics"]["corner_radius"] = serde_json::json!(250);
        json["metrics"]["item_spacing"] = serde_json::json!([1e9, -1e9]);
        let theme = Theme::parse("Mine", &json.to_string()).expect("it parses");
        assert_eq!(theme.metrics.body, 40.0, "a huge size is held to the largest drawn");
        assert_eq!(theme.metrics.small, 5.0, "a negative size is held to the smallest drawn");
        assert_eq!(theme.metrics.corner_radius, 32);
        assert_eq!(theme.metrics.item_spacing, [40.0, 0.0]);
        assert_eq!(theme.metrics.heading, 17.0, "what the file does not set stays standard");
    }

    #[test]
    fn a_colour_that_is_not_hexadecimal_is_refused_without_panicking() {
        // `#€€` is six bytes, so a byte cut would land inside a character and panic.
        for bad in ["#€€", "#€€€", "#€€€€", "#12345", "#1234567", "123456", "#GGGGGG", "#12345€"] {
            assert!(parse_colour(bad).is_err(), "{bad:?} is refused");
        }
        assert_eq!(parse_colour("#123456"), Ok(Color32::from_rgb(0x12, 0x34, 0x56)));
    }

    #[test]
    fn a_name_is_one_short_line() {
        let mut json: serde_json::Value = serde_json::from_str(&light().to_json().expect("json")).expect("json");
        json["name"] = serde_json::json!("Mine\nSecond line\twith a tab");
        let theme = Theme::parse("Mine", &json.to_string()).expect("it parses");
        assert_eq!(theme.name, "MineSecond linewith a tab", "a newline would break the button");

        json["name"] = serde_json::json!("N".repeat(200));
        let theme = Theme::parse("Mine", &json.to_string()).expect("it parses");
        assert_eq!(theme.name.chars().count(), MAX_NAME, "a long name is cut to fit");

        json["name"] = serde_json::json!("   ");
        let theme = Theme::parse("file-name", &json.to_string()).expect("it parses");
        assert_eq!(theme.name, "file-name", "a nameless file takes its own name");
    }

    #[test]
    fn a_file_too_large_to_be_a_theme_is_reported_not_read() {
        let mine = light().to_json().expect("json").replace("\"Light\"", "\"Mine\"");
        let dir = theme_folder("rustgis-theme-large-test", &[("mine.json", &mine)]);
        let huge = "x".repeat(MAX_THEME_BYTES as usize + 1);
        std::fs::write(dir.join("huge.json"), huge).expect("the file");
        let (found, errors) = Theme::available_in(Some(&dir));
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("huge.json"), "{errors:?}");
        assert!(found.iter().any(|t| t.name == "Mine"), "the good file still loads");
    }
}
