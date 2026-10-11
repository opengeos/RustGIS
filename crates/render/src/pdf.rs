//! A minimal PDF writer: each page is one image (the rendered layout) at the page's size.

use tiny_skia::Pixmap;

/// One PDF page: its image, already compressed, and the page size in millimetres. Pages are
/// compressed as they are made, so a long document never holds its uncompressed images at once.
pub struct Page {
    width: u32,
    height: u32,
    /// Zlib-compressed RGB pixels.
    rgb_z: Vec<u8>,
    pub width_mm: f64,
    pub height_mm: f64,
}

impl Page {
    /// Its compressed size in bytes.
    pub fn len(&self) -> usize {
        self.rgb_z.len()
    }
    pub fn is_empty(&self) -> bool {
        self.rgb_z.is_empty()
    }

    pub fn new(image: &Pixmap, width_mm: f64, height_mm: f64) -> Page {
        use miniz_oxide::deflate::core::{CompressorOxide, TDEFLFlush, TDEFLStatus, compress_to_output, create_comp_flags_from_zip_params};
        // RGB without the alpha (pages are drawn on white, so they're opaque), compressed a row
        // at a time so a large page never holds a second full-size copy of its pixels.
        let mut z = CompressorOxide::new(create_comp_flags_from_zip_params(6, 15, 0));
        let mut rgb_z = Vec::new();
        let mut row = Vec::with_capacity(image.width() as usize * 3);
        let mut ok = true;
        for line in image.pixels().chunks(image.width().max(1) as usize) {
            row.clear();
            row.extend(line.iter().flat_map(|px| {
                let c = px.demultiply();
                [c.red(), c.green(), c.blue()]
            }));
            let (status, used) = compress_to_output(&mut z, &row, TDEFLFlush::None, |b| {
                rgb_z.extend_from_slice(b);
                true
            });
            if used != row.len() || matches!(status, TDEFLStatus::BadParam | TDEFLStatus::PutBufFailed) {
                ok = false;
                break;
            }
        }
        let (status, _) = compress_to_output(&mut z, &[], TDEFLFlush::Finish, |b| {
            rgb_z.extend_from_slice(b);
            true
        });
        // Compressing into memory doesn't fail; if it ever did, the page is left blank (one
        // white pixel stretched over it) rather than holding a second full-size copy.
        let (width, height) = if ok && status == TDEFLStatus::Done {
            (image.width(), image.height())
        } else {
            rgb_z = miniz_oxide::deflate::compress_to_vec_zlib(&[255, 255, 255], 6);
            (1, 1)
        };
        // A page size that isn't a positive number would make the page box invalid.
        let mm = |v: f64| if v.is_finite() && v > 0.0 { v.min(5000.0) } else { 210.0 };
        Page { width, height, rgb_z, width_mm: mm(width_mm), height_mm: mm(height_mm) }
    }
}

/// Writes the pages as a PDF document.
pub fn write(pages: &[Page], title: &str) -> Vec<u8> {
    let mut out: Vec<u8> = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets: Vec<usize> = Vec::new();
    // Objects: 1 catalog, 2 pages, 3 info, then per page: page, contents, image.
    let n = pages.len();
    let obj = |out: &mut Vec<u8>, offsets: &mut Vec<usize>, body: &[u8]| {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", offsets.len()).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    };
    obj(&mut out, &mut offsets, b"<< /Type /Catalog /Pages 2 0 R >>");
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + i * 3)).collect();
    obj(&mut out, &mut offsets, format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" ")).as_bytes());
    obj(&mut out, &mut offsets, format!("<< /Title {} /Producer (RustGIS) >>", text_string(title)).as_bytes());
    for (i, p) in pages.iter().enumerate() {
        let (pw, ph) = (p.width_mm * 72.0 / 25.4, p.height_mm * 72.0 / 25.4);
        let (page, contents, image) = (4 + i * 3, 5 + i * 3, 6 + i * 3);
        obj(
            &mut out,
            &mut offsets,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {pw:.2} {ph:.2}] /Resources << /XObject << /Im{i} {image} 0 R >> >> /Contents {contents} 0 R >>"
            )
            .as_bytes(),
        );
        debug_assert_eq!(offsets.len(), page);
        let draw = format!("q {pw:.2} 0 0 {ph:.2} 0 0 cm /Im{i} Do Q");
        obj(&mut out, &mut offsets, format!("<< /Length {} >>\nstream\n{draw}\nendstream", draw.len()).as_bytes());
        let mut body = format!(
            "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /Length {} >>\nstream\n",
            p.width,
            p.height,
            p.rgb_z.len()
        )
        .into_bytes();
        body.extend_from_slice(&p.rgb_z);
        body.extend_from_slice(b"\nendstream");
        obj(&mut out, &mut offsets, &body);
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes());
    for o in &offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size {} /Root 1 0 R /Info 3 0 R >>\nstartxref\n{xref}\n%%EOF\n", offsets.len() + 1).as_bytes(),
    );
    out
}

/// A PDF text string: plain ASCII in parentheses, anything else as UTF-16BE hex.
fn text_string(s: &str) -> String {
    let s: String = s.chars().filter(|c| !c.is_control()).take(200).collect();
    if s.is_ascii() {
        let escaped: String = s.chars().flat_map(|c| if matches!(c, '(' | ')' | '\\') { vec!['\\', c] } else { vec![c] }).collect();
        format!("({escaped})")
    } else {
        let hex: String = s.encode_utf16().map(|u| format!("{u:04X}")).collect();
        format!("<FEFF{hex}>")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_two_page_pdf() {
        let mut a = Pixmap::new(20, 30).unwrap();
        a.fill(tiny_skia::Color::WHITE);
        let pdf = write(&[Page::new(&a, 210.0, 297.0), Page::new(&a, 297.0, 210.0)], "Atlas (draft)");
        let text = String::from_utf8_lossy(&pdf);
        assert!(text.starts_with("%PDF-1.4") && text.trim_end().ends_with("%%EOF"));
        assert!(text.contains("/Count 2") && text.contains("/MediaBox [0 0 595.28 841.89]") && text.contains("Atlas \\(draft\\)"));
        assert_eq!(text_string("España"), "<FEFF004500730070006100F10061>");
        // The xref offsets point at their objects.
        let find = |needle: &[u8]| pdf.windows(needle.len()).position(|w| w == needle).unwrap();
        let xref = find(b"\nxref\n") + 1;
        let tail = std::str::from_utf8(&pdf[xref..]).unwrap();
        let off: usize = tail.lines().nth(3).unwrap()[..10].parse().unwrap();
        assert!(pdf[off..].starts_with(b"1 0 obj"));
        let off: usize = tail.lines().nth(6).unwrap()[..10].parse().unwrap();
        assert!(pdf[off..].starts_with(b"4 0 obj"));
    }

    #[test]
    fn pages_compress_row_by_row_and_sanitise_their_size() {
        let mut a = Pixmap::new(7, 5).unwrap();
        a.fill(tiny_skia::Color::from_rgba8(10, 200, 30, 255));
        let p = Page::new(&a, f64::NAN, 0.0);
        let rgb = miniz_oxide::inflate::decompress_to_vec_zlib(&p.rgb_z).unwrap();
        assert_eq!(rgb.len(), 7 * 5 * 3);
        assert!(rgb.chunks(3).all(|c| c == [10, 200, 30]));
        assert_eq!((p.width_mm, p.height_mm), (210.0, 210.0));
    }
}
