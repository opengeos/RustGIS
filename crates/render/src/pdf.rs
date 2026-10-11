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
    pub fn new(image: &Pixmap, width_mm: f64, height_mm: f64) -> Page {
        // Opaque RGB: the page is white, so premultiplied pixels are already composited on it.
        let rgb: Vec<u8> = image.pixels().iter().flat_map(|px| [px.red(), px.green(), px.blue()]).collect();
        Page {
            width: image.width(),
            height: image.height(),
            rgb_z: miniz_oxide::deflate::compress_to_vec_zlib(&rgb, 6),
            width_mm,
            height_mm,
        }
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
    obj(&mut out, &mut offsets, format!("<< /Title ({}) /Producer (RustGIS) >>", escape(title)).as_bytes());
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

fn escape(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii() && !c.is_ascii_control())
        .take(200)
        .flat_map(|c| match c {
            '(' | ')' | '\\' => vec!['\\', c],
            c => vec![c],
        })
        .collect()
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
        // The xref offsets point at their objects.
        let find = |needle: &[u8]| pdf.windows(needle.len()).position(|w| w == needle).unwrap();
        let xref = find(b"\nxref\n") + 1;
        let tail = std::str::from_utf8(&pdf[xref..]).unwrap();
        let off: usize = tail.lines().nth(3).unwrap()[..10].parse().unwrap();
        assert!(pdf[off..].starts_with(b"1 0 obj"));
        let off: usize = tail.lines().nth(6).unwrap()[..10].parse().unwrap();
        assert!(pdf[off..].starts_with(b"4 0 obj"));
    }
}
