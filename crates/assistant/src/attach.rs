//! Pictures the user attaches to a message: a reference map, a screenshot of a problem, a photo of
//! a sketch. [`prepare`] reads one whichever way it came (the pane's picker or a paste, the command
//! line's `--image`), and makes it ready to send to any provider: decoded within limits, turned
//! upright, drawn smaller when it is larger than models read, and encoded again, which leaves out
//! whatever the file carried besides its pixels (a photo's location among them).
//!
//! Plain functions without I/O, as the rest of the crate: the front end reads the file.

use std::io::Cursor;

use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits};

/// The largest file a picture is read from; a front end refuses a larger one before reading it.
pub const MAX_FILE_BYTES: usize = 20 * 1024 * 1024;

/// The most pictures one message takes.
pub const MAX_PER_MESSAGE: usize = 4;

/// The longest side a picture is sent at: what Claude reads before it scales a picture down
/// itself, and within what the other providers take.
pub const LONGEST_SIDE: u32 = 1568;

/// The largest picture read: a phone's 50-megapixel photo fits, and decoding one never takes more
/// than [`MAX_DECODE_BYTES`].
const MAX_DECODED_SIDE: u32 = 8192;
const MAX_DECODE_BYTES: u64 = 320 * 1024 * 1024;

/// The most a picture sent may weigh, as `view_map`'s: 5 MiB once base64-encoded.
const MAX_SENT_BYTES: usize = crate::tools::MAX_PNG_BYTES;

/// The longest name a picture keeps, in characters.
const MAX_NAME: usize = 80;

/// The file types a picture is read from, for a picker's filter.
pub const EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp"];

/// A picture ready to go with the user's message.
#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    /// The file it came from (or "pasted picture"), for the model and the transcript.
    pub name: String,
    /// The picture as sent: a PNG when it has transparency, else a JPEG.
    pub data: Vec<u8>,
    pub mime: &'static str,
    /// Its size in pixels as sent.
    pub width: u32,
    pub height: u32,
    /// Its size in pixels as given, larger when it was drawn smaller to send.
    pub original: (u32, u32),
}

impl Picture {
    /// What stands in its place in the conversation once it's no longer sent.
    pub fn caption(&self) -> String {
        format!("[a picture the user attached, {}, {} × {}, no longer attached]", self.name, self.width, self.height)
    }

    /// Whether it is sent smaller than it was given.
    pub fn shrunk(&self) -> bool {
        (self.width, self.height) != self.original
    }

    pub(crate) fn base64(&self) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(&self.data)
    }

    pub(crate) fn data_url(&self) -> String {
        format!("data:{};base64,{}", self.mime, self.base64())
    }
}

/// A picture from the bytes of a PNG, JPEG or WebP file named `name`. Refused, saying why: a file
/// over [`MAX_FILE_BYTES`], another format, one that doesn't decode, or one over
/// [`MAX_DECODED_SIDE`] pixels a side.
///
/// Decoding is the long step (up to about a second for the largest photo): the desktop runs it off
/// the window's thread.
pub fn prepare(name: &str, bytes: &[u8]) -> Result<Picture, String> {
    let name = clean_name(name);
    if bytes.len() > MAX_FILE_BYTES {
        return Err(format!("{name}: a picture can be at most {} MiB", MAX_FILE_BYTES / 1024 / 1024));
    }
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(|e| format!("{name}: {e}"))?;
    if !matches!(reader.format(), Some(ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP)) {
        return Err(format!("{name}: a picture must be a PNG, JPEG or WebP file"));
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DECODED_SIDE);
    limits.max_image_height = Some(MAX_DECODED_SIDE);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    let unreadable = |e: image::ImageError| match e {
        image::ImageError::Limits(_) => format!("{name}: a picture can be at most {MAX_DECODED_SIDE} pixels a side"),
        e => format!("{name}: the picture can't be read ({e})"),
    };
    let mut decoder = reader.into_decoder().map_err(unreadable)?;
    // A photo taken with the phone on its side says so; an unreadable note is no turn.
    let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder).map_err(unreadable)?;
    img.apply_orientation(orientation);
    finish(name, img)
}

/// A picture from `width` × `height` pixels of RGBA, row by row (a pasted screenshot).
pub fn from_rgba(name: &str, width: u32, height: u32, rgba: Vec<u8>) -> Result<Picture, String> {
    let name = clean_name(name);
    if width > MAX_DECODED_SIDE || height > MAX_DECODED_SIDE {
        return Err(format!("{name}: a picture can be at most {MAX_DECODED_SIDE} pixels a side"));
    }
    let img =
        image::RgbaImage::from_raw(width, height, rgba).ok_or_else(|| format!("{name}: the picture's pixels don't match its size"))?;
    finish(name, DynamicImage::ImageRgba8(img))
}

/// Draws `img` no larger than [`LONGEST_SIDE`] and encodes it, smaller again while it weighs more
/// than [`MAX_SENT_BYTES`].
fn finish(name: String, img: DynamicImage) -> Result<Picture, String> {
    let original = (img.width(), img.height());
    if original.0 == 0 || original.1 == 0 {
        return Err(format!("{name}: the picture is empty"));
    }
    let fit = |side: u32| {
        if original.0.max(original.1) > side { img.resize(side, side, image::imageops::FilterType::Triangle) } else { img.clone() }
    };
    let mut side = LONGEST_SIDE;
    let mut fitted = fit(side);
    // Opaque pixels go as a JPEG, far lighter for a photo; transparency needs a PNG. Looked at
    // once drawn small, so a large picture isn't copied whole to look.
    let alpha = fitted.color().has_alpha() && fitted.to_rgba8().pixels().any(|p| p.0[3] < 255);
    loop {
        let (data, mime) = encode(&fitted, alpha).map_err(|e| format!("{name}: {e}"))?;
        if data.len() <= MAX_SENT_BYTES || side <= 256 {
            if data.len() > MAX_SENT_BYTES {
                return Err(format!("{name}: the picture is too heavy to send, even drawn small"));
            }
            return Ok(Picture { name, data, mime, width: fitted.width(), height: fitted.height(), original });
        }
        side = side * 3 / 4;
        fitted = fit(side);
    }
}

fn encode(img: &DynamicImage, alpha: bool) -> Result<(Vec<u8>, &'static str), String> {
    let mut out = Vec::new();
    if alpha {
        img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png).map_err(|e| e.to_string())?;
        Ok((out, "image/png"))
    } else {
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85);
        img.to_rgb8().write_with_encoder(encoder).map_err(|e| e.to_string())?;
        Ok((out, "image/jpeg"))
    }
}

/// The file's name without its folders or control characters, at most [`MAX_NAME`] characters.
fn clean_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let clean: String = base.chars().filter(|c| !c.is_control()).take(MAX_NAME).collect();
    let clean = clean.trim();
    if clean.is_empty() { "picture".into() } else { clean.into() }
}
