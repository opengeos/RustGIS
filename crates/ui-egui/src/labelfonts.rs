//! Label fonts in the map view: a label class's installed family (`LabelClass::font`), loaded into
//! egui when a label first asks for it, as the export draws it. The face is the one the renderer
//! measured the label in (`families::pick`), so a label sits alike on screen and in an export.
//! Until egui has it (fonts set during a frame take effect on the next), the label draws in the
//! interface font; a later reinstall of the interface fonts drops it, and it is loaded again.

use std::collections::HashMap;
use std::sync::Arc;

use egui::{FontData, FontFamily};
use rustgis_engine::render::families;

/// Fonts asked for, by egui family name: the pass each was set in, or `None` when its file
/// couldn't be read (not asked for again).
#[derive(Clone, Default)]
struct Requests(HashMap<String, Option<u64>>);

/// Fonts asked for this pass, not yet set.
#[derive(Clone, Default)]
struct Pending(Vec<(String, &'static [u8], u32, bool)>);

/// The egui family a label in installed family `font` draws in, `bold` and `italic`, and whether
/// egui slants it (the family has no italic face). `None` for the interface font: no family, one
/// not installed, or one not loaded yet (asked for here, so a later frame has it).
pub(crate) fn family(ctx: &egui::Context, font: Option<u16>, bold: bool, italic: bool) -> Option<(FontFamily, bool)> {
    let fam = families::installed_now()?.get(usize::from(font?))?;
    let (face, slanted) = families::pick(fam, bold, italic)?;
    let heavier = families::heavier(face, bold);
    let key = format!("label-font:{}#{}{}", face.path.display(), face.index, if heavier { ":bold" } else { "" });
    let family = FontFamily::Name(key.as_str().into());
    if ctx.fonts(|f| f.definitions().families.contains_key(&family)) {
        return Some((family, slanted));
    }
    let id = egui::Id::unique("label-fonts");
    let pass = ctx.cumulative_pass_nr();
    let asked = ctx.data_mut(|d| d.get_temp_mut_or_default::<Requests>(id).0.get(&key).copied());
    match asked {
        // Its file couldn't be read.
        Some(None) => return None,
        // Set a pass ago: it takes effect on the next. Set before that and gone: the interface
        // fonts were installed over it.
        Some(Some(p)) if pass <= p + 1 => return None,
        _ => {}
    }
    let bytes = families::bytes(face);
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<Requests>(id).0.insert(key.clone(), bytes.map(|_| pass));
        if let Some(b) = bytes {
            d.get_temp_mut_or_default::<Pending>(id).0.push((key, b, face.index, heavier));
        }
    });
    // A frame to set it, and one more drawn in it.
    ctx.request_repaint();
    None
}

/// Sets the label fonts the last pass asked for, in one go, each with the interface fonts after it
/// for the characters it lacks. Called once a frame, before the labels are drawn and after the
/// interface fonts are installed.
pub(crate) fn install_pending(ctx: &egui::Context) {
    let id = egui::Id::unique("label-fonts");
    let pending = ctx.data_mut(|d| std::mem::take(&mut d.get_temp_mut_or_default::<Pending>(id).0));
    if pending.is_empty() {
        return;
    }
    let mut defs = ctx.fonts(|f| f.definitions().clone());
    let fallback: Vec<String> = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    for (key, bytes, index, heavier) in pending {
        let mut data = FontData::from_static(bytes);
        data.index = index;
        if heavier {
            let weight = egui::epaint::text::VariationCoords::new([(b"wght", families::BOLD_WEIGHT)]);
            data = data.tweak(egui::FontTweak { coords: weight, ..Default::default() });
        }
        defs.font_data.insert(key.clone(), Arc::new(data));
        let mut list = vec![key.clone()];
        list.extend(fallback.iter().cloned());
        defs.families.insert(FontFamily::Name(key.as_str().into()), list);
    }
    ctx.set_fonts(defs);
}
