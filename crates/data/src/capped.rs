//! A file made in memory under a size limit, such as a download in the browser: its writer fails
//! as soon as the file would pass the limit, so it is never held larger.

use std::io::{Error, ErrorKind, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::{DataError, Result};

/// The most bytes the buffers a file is made in may hold in all (a Shapefile's parts share one).
#[derive(Debug)]
pub struct Limit {
    max: usize,
    held: AtomicUsize,
    passed: AtomicBool,
}

impl Limit {
    /// A limit of `max` bytes.
    pub const fn new(max: usize) -> Self {
        Limit { max, held: AtomicUsize::new(0), passed: AtomicBool::new(false) }
    }

    /// No limit but memory: for a file that is to be written.
    pub const fn none() -> Self {
        Limit::new(usize::MAX)
    }

    /// Whether a write was refused for passing the limit.
    pub fn passed(&self) -> bool {
        self.passed.load(Ordering::Relaxed)
    }

    /// An empty buffer counted against this limit.
    pub(crate) fn buffer(&self) -> Buffer<'_> {
        Buffer { file: Vec::new(), at: 0, limit: self }
    }

    /// Refuses a file of `bytes` made apart from the buffers (a zip of them) when it passes the
    /// limit.
    pub(crate) fn admit(&self, bytes: usize) {
        if bytes > self.max {
            self.passed.store(true, Ordering::Relaxed);
        }
    }

    /// `made`, or an error once a write was refused: a writer may drop that write's error (some
    /// finish their file as they are dropped), so this, not its result, says whether the file is
    /// whole.
    pub(crate) fn whole<T>(&self, made: Result<T>) -> Result<T> {
        if self.passed() {
            return Err(DataError::Format(format!("the file passes the {} bytes it may come to", self.max)));
        }
        made
    }

    /// Counts `n` more bytes held, refusing them past the limit, and gives how many are held.
    fn take(&self, n: usize) -> std::io::Result<usize> {
        let before = self.held.fetch_add(n, Ordering::Relaxed);
        if let Some(held) = before.checked_add(n).filter(|held| *held <= self.max) {
            return Ok(held);
        }
        self.held.fetch_sub(n, Ordering::Relaxed);
        self.passed.store(true, Ordering::Relaxed);
        Err(Error::other("the file passes the size it may come to"))
    }
}

/// A file made in memory within a [`Limit`], written and sought as a file is.
#[derive(Debug)]
pub(crate) struct Buffer<'a> {
    file: Vec<u8>,
    /// Where the next write goes.
    at: u64,
    limit: &'a Limit,
}

impl Buffer<'_> {
    /// The file made, no longer counted against the limit.
    pub(crate) fn into_inner(self) -> Vec<u8> {
        self.limit.held.fetch_sub(self.file.len(), Ordering::Relaxed);
        self.file
    }

    /// Makes room for the file to reach `end` bytes, within the limit: its allocation doubles,
    /// but never past what the limit leaves it, and a refusal by the system is an error.
    fn reserve(&mut self, end: usize) -> std::io::Result<()> {
        let Some(more) = end.checked_sub(self.file.len()).filter(|n| *n > 0) else { return Ok(()) };
        let held = self.limit.take(more)?;
        if end > self.file.capacity() {
            let most = end.saturating_add(self.limit.max - held);
            let room = self.file.capacity().saturating_mul(2).clamp(end, most);
            self.file.try_reserve_exact(room - self.file.len()).map_err(|_| Error::from(ErrorKind::OutOfMemory))?;
        }
        Ok(())
    }
}

impl Write for Buffer<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let too_far = || Error::from(ErrorKind::InvalidInput);
        let at = usize::try_from(self.at).map_err(|_| too_far())?;
        let end = at.checked_add(bytes.len()).ok_or_else(too_far)?;
        if bytes.is_empty() {
            return Ok(0);
        }
        self.reserve(end)?;
        if at == self.file.len() {
            self.file.extend_from_slice(bytes);
        } else {
            // Over what is there, or past the end, the gap left zero as in a file.
            if end > self.file.len() {
                self.file.resize(end, 0);
            }
            self.file.get_mut(at..end).ok_or_else(too_far)?.copy_from_slice(bytes);
        }
        self.at = end as u64;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Seek for Buffer<'_> {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let at = match to {
            SeekFrom::Start(at) => Some(at),
            SeekFrom::End(by) => (self.file.len() as u64).checked_add_signed(by),
            SeekFrom::Current(by) => self.at.checked_add_signed(by),
        };
        self.at = at.ok_or_else(|| Error::from(ErrorKind::InvalidInput))?;
        Ok(self.at)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;

    use super::*;
    use crate::raster::{ColorHint, Crs, GeoTransform, Raster, cogwrite, geotiff};
    use crate::{Dataset, Field, FieldType, Geometry, GeometryType, Value};

    /// A buffer holds what a file would after seeks and gaps, never allocates past its limit, and
    /// once a write is refused the limit says so, whatever the writer made of the refusal.
    #[test]
    fn a_buffer_is_a_file_that_stops_at_its_limit() {
        let limit = Limit::new(10);
        let mut b = limit.buffer();
        b.write_all(b"abcd").unwrap();
        b.seek(SeekFrom::Start(1)).unwrap();
        b.write_all(b"XY").unwrap();
        b.seek(SeekFrom::End(2)).unwrap();
        b.write_all(b"z").unwrap();
        assert_eq!(b.file, b"aXYd\0\0z");
        // Another buffer shares what is left.
        let mut other = limit.buffer();
        other.write_all(b"123").unwrap();
        assert!(!limit.passed());
        assert!(other.write_all(b"4").is_err());
        assert!(limit.passed() && b.file.capacity() <= 10, "{}", b.file.capacity());
        let e = limit.whole(Ok(())).unwrap_err().to_string();
        assert!(e.contains("passes the 10 bytes"), "{e}");
        assert!(b.seek(SeekFrom::Current(-100)).is_err());
        let limit = Limit::new(1000);
        let mut b = limit.buffer();
        for _ in 0..100 {
            b.write_all(b"ten bytes.").unwrap();
            assert!(b.file.capacity() <= 1000, "{}", b.file.capacity());
        }
        assert_eq!(b.into_inner().len(), 1000);
        limit.buffer().write_all(&[0; 1000]).unwrap();
        assert!(!limit.passed(), "a file handed over is no longer counted");
    }

    /// Counts the items a writer says it has done.
    #[derive(Default)]
    struct Done(AtomicU64);

    impl crate::progress::Probe for Done {
        fn cancelled(&self) -> bool {
            false
        }
        fn begin(&self, _: u64) {}
        fn advance(&self, n: u64) {
            self.0.fetch_add(n, Ordering::Relaxed);
        }
    }

    /// Each vector format a download is made in stops within the first features that pass the
    /// limit, and under it gives the bytes the writer without one gives.
    #[test]
    fn every_download_format_stops_at_the_limit() {
        const ROWS: u64 = 5_000;
        let fields = vec![Field::new("name", FieldType::Text)];
        let mut ds = Dataset::new("Places", GeometryType::Point, fields);
        for i in 0..ROWS {
            let at = crate::Coord::new((i % 360) as f64 - 180.0, (i % 170) as f64 - 85.0);
            ds.push(Some(Geometry::Point(at)), vec![Value::Text(format!("place {i}"))]);
        }
        let formats = ["geojson", "csv", "kml", "kmz", "gpx", "shp", "parquet"];
        let whole = |ext: &str, limit: &Limit| crate::io::write_bytes_noted_within(&ds, ext, Crs::LonLat, "out", limit).map(|w| w.0);
        let text = |format: &str, limit: &Limit| crate::io::text_bytes_within(&ds, format, Some(0), limit);
        let check = |name: &str, early: bool, make: &dyn Fn(&Limit) -> Result<Vec<u8>>| {
            let unlimited = make(&Limit::none()).unwrap();
            assert_eq!(make(&Limit::new(1 << 24)).unwrap(), unlimited, "{name}");
            let done = Arc::new(Done::default());
            let _probe = crate::progress::install(Some(done.clone()));
            let limit = Limit::new(200);
            assert!(make(&limit).is_err() && limit.passed(), "{name}");
            let done = done.0.load(Ordering::Relaxed);
            // A GeoParquet writes a group of rows at a time.
            assert!(!early || done < ROWS / 2, "{name}: {done} of {ROWS} features before it stopped");
        };
        for ext in formats {
            check(ext, ext != "parquet", &|limit| whole(ext, limit));
        }
        for format in ["geojson", "csv", "kml", "gpx", "wkt"] {
            check(format, true, &|limit| text(format, limit));
        }
        // A KMZ and a zipped Shapefile are held to the limit before they are zipped, though their
        // zips would fit.
        for (ext, parts) in [("kmz", &["kml"][..]), ("shp", &["shp", "shx", "dbf"])] {
            let zip = whole(ext, &Limit::none()).unwrap();
            let counted = |name: &str| parts.iter().any(|p| name.ends_with(p));
            let unzipped: usize = crate::zip::read(&zip).unwrap().iter().filter(|e| counted(&e.0)).map(|e| e.1.len()).sum();
            assert!(zip.len() < unzipped - 1, "{ext}");
            assert!(whole(ext, &Limit::new(unzipped - 1)).is_err(), "{ext}");
        }
    }

    /// A GeoTIFF and a cloud-optimised one stop at the limit, and under it are unchanged.
    #[test]
    fn a_geotiff_download_stops_at_the_limit() {
        let transform = GeoTransform { origin_x: 0.0, origin_y: 2.0, pixel_width: 0.001, pixel_height: -0.001 };
        let cells: Vec<f32> = (0..1_000_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 8) as f32).collect();
        let r = Raster::new("r", 1000, 1000, vec![cells], transform, Crs::LonLat, None, ColorHint::Gray, "Float32").unwrap();
        for cog in [false, true] {
            let make = |limit: &Limit| {
                if cog {
                    cogwrite::write_with_info_within(&r, 0, limit).map(|(bytes, _)| bytes)
                } else {
                    geotiff::write_within(&r, 0, limit)
                }
            };
            let unlimited = make(&Limit::none()).unwrap();
            assert_eq!(make(&Limit::new(unlimited.len())).unwrap(), unlimited, "cog {cog}");
            let done = Arc::new(Done::default());
            let _probe = crate::progress::install(Some(done.clone()));
            let limit = Limit::new(1000);
            assert!(make(&limit).is_err() && limit.passed(), "cog {cog}");
            // At the first piece of the image, of the four or more.
            let done = done.0.load(Ordering::Relaxed);
            assert!(done <= 1, "cog {cog}: {done} pieces");
        }
    }
}
