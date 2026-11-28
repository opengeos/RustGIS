//! A layout picture's image and the file it is read from: the one reader both commands use, and
//! what they refuse.

use super::*;

/// `image` or `path` given to an element that isn't a picture.
pub(in crate::cmd::layout) const ONLY_PICTURE_FILE: &str = "only pictures take an image or a path";
/// A `path` that isn't text, or is blank.
pub(in crate::cmd::layout) const PICTURE_PATH: &str = "path is the path of a PNG or JPEG file";
/// An `image` that isn't text.
pub(in crate::cmd::layout) const PICTURE_IMAGE: &str = "image is a PNG or JPEG image in base64";
/// A `path` that is an address, which is never tried as a file nor fetched, and never repeated
/// (it could carry a login).
pub(in crate::cmd::layout) const PICTURE_ADDRESS: &str = "a picture is read from a file on this computer, not a web address";
/// A `path` given in the browser.
pub(in crate::cmd::layout) const PICTURE_PATH_IN_BROWSER: &str =
    "a picture's path can't be read in the browser, which has no file paths: give its image";
/// Both `image` and `path` given.
pub(in crate::cmd::layout) const PICTURE_BOTH: &str = "give a picture an image or a path, not both";

/// A picture whose header reads but whose image can't be drawn: damaged, declaring too many
/// pixels a side, or needing more memory once decoded than the decoder may take.
pub(in crate::cmd::layout) fn picture_undrawn() -> String {
    format!(
        "the picture can't be drawn: its image is damaged, or too large (over {} pixels a side, or over {} MB once decoded)",
        rustgis_render::layout::PICTURE_MAX_DECLARED,
        rustgis_render::layout::PICTURE_MAX_DECODED_MB
    )
}

/// A picture read from a file whose path isn't kept as its file.
pub(in crate::cmd::layout) fn picture_unremembered() -> String {
    format!(
        "the picture is read, but its file isn't remembered: a path with control or direction characters, over {} bytes or \
         without a file name isn't kept",
        rustgis_map::layout::MAX_PICTURE_SOURCE
    )
}

/// A picture that remembered its file given an image by `layout.update`: the file is forgotten.
pub(in crate::cmd::layout) const PICTURE_FORGOTTEN: &str = "the picture's file is no longer remembered: its image was given directly";

/// Says each of [`rustgis_render::layout::pictures_notices`] of `gaps`, the pictures an export
/// draws as a grey box, with `prefix` before it.
pub(in crate::cmd) fn say_pictures_not_drawn(s: &mut Session, gaps: &[rustgis_render::layout::PictureGap], prefix: &str) {
    for note in rustgis_render::layout::pictures_notices(gaps) {
        s.warn(format!("{prefix}{note}"));
    }
}

/// A picture's image as a command gives it, and the file it was read from.
pub(crate) struct GivenPicture {
    /// The image, base64, checked to draw.
    pub image: String,
    /// The path it was read from, when [`rustgis_map::layout::picture_source_kept`] keeps it.
    pub source: Option<String>,
    /// Read from a file whose path isn't kept: said once by the command.
    pub unremembered: bool,
    /// Given as an image, not read from a file: a picture that remembered its file forgets it.
    pub given_image: bool,
}

/// A command's `path`: none when absent or null, as given when it is text that isn't blank and
/// isn't an address ([`rustgis_map::layout::picture_path_is_address`]); refused otherwise, and
/// in the browser, which has no file paths, refused before anything is read
/// ([`PICTURE_PATH_IN_BROWSER`]).
pub(crate) fn picture_path_param(p: &Value) -> Result<Option<&str>, String> {
    picture_path_param_in(p, cfg!(target_arch = "wasm32"))
}

/// [`picture_path_param`] in the browser (`web`) or elsewhere.
fn picture_path_param_in(p: &Value, web: bool) -> Result<Option<&str>, String> {
    match p.get("path") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(path)) if path.trim().is_empty() => Err(PICTURE_PATH.into()),
        Some(Value::String(_)) if web => Err(PICTURE_PATH_IN_BROWSER.into()),
        Some(Value::String(path)) if rustgis_map::layout::picture_path_is_address(path) => Err(PICTURE_ADDRESS.into()),
        Some(Value::String(path)) => Ok(Some(path)),
        Some(_) => Err(PICTURE_PATH.into()),
    }
}

/// Whether a command gives a picture an `image` or a `path` (present and not null).
pub(crate) fn gives_picture(p: &Value) -> bool {
    ["image", "path"].iter().any(|k| p.get(*k).is_some_and(|v| !v.is_null()))
}

/// The picture a command's `image` or `path` gives, none when it gives neither; refused when both
/// are given, either is of the wrong kind, the file can't be read or isn't a PNG or JPEG of at most
/// 16 MiB, or the image can't be drawn.
///
/// Runs on the command's thread (the UI thread in the app): bounded work, not work that grows with
/// a layer. The file is read whole, as `layout.addElement` always read it, at most 16 MiB
/// (`graphics::MAX_PICTURE`). The image is then decoded once, here, because whether it draws
/// decides whether the command is taken: one that can't be drawn is refused, the layout left as it
/// was, rather than added or kept as a grey box. The decoder doesn't start on an image declaring
/// over [`rustgis_render::layout::PICTURE_MAX_DECLARED`] pixels a side and allocates at most
/// [`rustgis_render::layout::PICTURE_MAX_DECODED_MB`] MiB. The decoded copy stays in the cache of
/// decoded pictures of the thread that decoded it ([`rustgis_render::layout::picture_draws`]): the
/// Layout view draws on the same thread and finds it there; an export the app writes on another
/// thread decodes it again, once, on that thread.
pub(in crate::cmd::layout) fn picture_given(p: &Value) -> Result<Option<GivenPicture>, String> {
    let given = |k: &str| p.get(k).filter(|v| !v.is_null());
    if given("image").is_some() && given("path").is_some() {
        return Err(PICTURE_BOTH.into());
    }
    let picture = match (given("image"), picture_path_param(p)?) {
        (Some(Value::String(image)), _) => {
            GivenPicture {
                image: crate::cmd::graphics::checked_picture(image.trim().to_string())?,
                source: None,
                unremembered: false,
                given_image: true,
            }
        }
        (Some(_), _) => return Err(PICTURE_IMAGE.into()),
        (None, Some(path)) => {
            let image = crate::cmd::graphics::picture_file(path)?;
            let kept = rustgis_map::layout::picture_source_kept(path);
            GivenPicture { image, source: kept.then(|| path.to_string()), unremembered: !kept, given_image: false }
        }
        (None, None) => return Ok(None),
    };
    if !rustgis_render::layout::picture_draws(&picture.image) {
        return Err(picture_undrawn());
    }
    Ok(Some(picture))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In the browser a path is refused before anything is read, saying why and what to give
    /// instead; elsewhere it is taken as given. Blank, null and the wrong kind answer as anywhere.
    #[test]
    fn a_path_is_refused_in_the_browser_before_it_is_read() {
        let browser = "a picture's path can't be read in the browser, which has no file paths: give its image";
        for path in ["logo.png", "/no/such/folder/logo.png", "https://example.invalid/logo.png"] {
            let p = serde_json::json!({"path": path});
            assert_eq!(picture_path_param_in(&p, true), Err(browser.to_string()), "{path}");
        }
        assert_eq!(picture_path_param_in(&serde_json::json!({"path": "logo.png"}), false), Ok(Some("logo.png")));
        for web in [true, false] {
            assert_eq!(picture_path_param_in(&serde_json::json!({"path": null}), web), Ok(None));
            assert_eq!(picture_path_param_in(&serde_json::json!({}), web), Ok(None));
            for blank in [serde_json::json!("  "), serde_json::json!(5)] {
                assert_eq!(picture_path_param_in(&serde_json::json!({"path": blank}), web), Err(PICTURE_PATH.to_string()));
            }
        }
    }
}
