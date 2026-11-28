//! Zip archives (Deflate or stored): packages bundle a project or map with its data files.

use crate::capped::Limit;

/// Most entries read from an archive (plain zip holds at most 65,535; ZIP64 isn't read).
const MAX_ENTRIES: usize = 65_535;
/// Largest entry inflated (1 GiB).
const MAX_ENTRY: usize = 1 << 30;
/// Most bytes inflated from one archive, all entries together (less in the browser).
pub const MAX_TOTAL: usize = if cfg!(target_arch = "wasm32") { 256 << 20 } else { 1 << 30 };

/// The CRC-32 lookup table, built at compile time.
const CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
};

fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for b in data {
        c = CRC_TABLE.get(((c ^ *b as u32) & 0xFF) as usize).copied().unwrap_or(0) ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

/// Writes a zip archive of (name, bytes) entries, Deflate-compressed. Plain zip (no Zip64)
/// holds at most 65,535 entries and 4 GiB, and names of up to 65,535 bytes; beyond that it's an
/// error rather than a damaged archive.
pub fn write(entries: &[(String, Vec<u8>)]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    write_to(entries, &mut out)?;
    Ok(out)
}

/// [`write`]'s archive of `entries` within `limit`: the entries, held already, stay counted
/// against it while the archive is made, and the archive is written within what they leave, so
/// the two together never pass it (besides one entry's compressed bytes at a time).
pub fn write_within(entries: &[(String, Vec<u8>)], limit: &Limit) -> crate::Result<Vec<u8>> {
    let parts: usize = entries.iter().map(|(_, bytes)| bytes.capacity()).sum();
    let _parts = limit.hold(parts)?;
    limit.file(|out| write_to(entries, out).map_err(crate::DataError::Format))
}

/// [`write`]'s archive, written to `out` as it is made (a buffer held to a limit stops it there),
/// the central directory too. Besides `out`, it holds a few numbers for each entry, and one
/// entry's compressed bytes at a time.
fn write_to(entries: &[(String, Vec<u8>)], out: &mut (impl std::io::Write + ?Sized)) -> Result<(), String> {
    let too_big = || "the archive is too large for a zip file".to_string();
    let failed = |e: std::io::Error| e.to_string();
    let count = u16::try_from(entries.len()).map_err(|_| too_big())?;
    // How much of the archive is written: where the next entry starts.
    let mut written: usize = 0;
    // What each entry's central record repeats of it.
    let mut records = Vec::with_capacity(entries.len());
    for (name, data) in entries {
        let n = name.as_bytes();
        let name_len = u16::try_from(n.len()).map_err(|_| too_big())?;
        let data_len = u32::try_from(data.len()).map_err(|_| too_big())?;
        let crc = crc32(data);
        let z = miniz_oxide::deflate::compress_to_vec(data, 6);
        let (method, body): (u16, &[u8]) = if z.len() < data.len() { (8, &z) } else { (0, data) };
        let offset = u32::try_from(written).map_err(|_| too_big())?;
        let body_len = u32::try_from(body.len()).map_err(|_| too_big())?;
        let record = Record { method, crc, body_len, data_len, name_len, offset };
        let local = record.header(n, false);
        out.write_all(&local).map_err(failed)?;
        out.write_all(body).map_err(failed)?;
        written = written.saturating_add(local.len()).saturating_add(body.len());
        records.push(record);
    }
    let cd_offset = u32::try_from(written).map_err(|_| too_big())?;
    // A record at a time: a directory of many entries is never held apart from `out`.
    let mut cd_size: usize = 0;
    for ((name, _), record) in entries.iter().zip(&records) {
        let central = record.header(name.as_bytes(), true);
        out.write_all(&central).map_err(failed)?;
        cd_size = cd_size.saturating_add(central.len());
    }
    let cd_len = u32::try_from(cd_size).map_err(|_| too_big())?;
    let mut end = Vec::new();
    end.extend_from_slice(&0x0605_4B50u32.to_le_bytes());
    end.extend_from_slice(&[0, 0, 0, 0]);
    end.extend_from_slice(&count.to_le_bytes());
    end.extend_from_slice(&count.to_le_bytes());
    end.extend_from_slice(&cd_len.to_le_bytes());
    end.extend_from_slice(&cd_offset.to_le_bytes());
    end.extend_from_slice(&0u16.to_le_bytes());
    out.write_all(&end).map_err(failed)
}

/// An entry as its local header and its central record describe it.
struct Record {
    method: u16,
    crc: u32,
    body_len: u32,
    data_len: u32,
    name_len: u16,
    offset: u32,
}

impl Record {
    /// The entry's central record, or its local header, followed by its name `n`.
    fn header(&self, n: &[u8], central: bool) -> Vec<u8> {
        let mut h = Vec::new();
        h.extend_from_slice(&if central { 0x0201_4B50u32 } else { 0x0403_4B50 }.to_le_bytes());
        if central {
            h.extend_from_slice(&20u16.to_le_bytes()); // made by
        }
        h.extend_from_slice(&20u16.to_le_bytes()); // needed to extract
        h.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 names
        h.extend_from_slice(&self.method.to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes()); // time
        h.extend_from_slice(&0x21u16.to_le_bytes()); // date: 1980-01-01
        h.extend_from_slice(&self.crc.to_le_bytes());
        h.extend_from_slice(&self.body_len.to_le_bytes());
        h.extend_from_slice(&self.data_len.to_le_bytes());
        h.extend_from_slice(&self.name_len.to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes()); // extra
        if central {
            h.extend_from_slice(&0u16.to_le_bytes()); // comment
            h.extend_from_slice(&0u16.to_le_bytes()); // disk
            h.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
            h.extend_from_slice(&0u32.to_le_bytes()); // external attributes
            h.extend_from_slice(&self.offset.to_le_bytes());
        }
        h.extend_from_slice(n);
        h
    }
}

fn u16_at(b: &[u8], i: usize) -> Option<usize> {
    let s = b.get(i..i.checked_add(2)?)?;
    Some(u16::from_le_bytes([*s.first()?, *s.get(1)?]) as usize)
}

fn u32_at(b: &[u8], i: usize) -> Option<usize> {
    let s = b.get(i..i.checked_add(4)?)?;
    usize::try_from(u32::from_le_bytes([*s.first()?, *s.get(1)?, *s.get(2)?, *s.get(3)?])).ok()
}

fn u64_at(b: &[u8], i: usize) -> Option<usize> {
    let s = b.get(i..i.checked_add(8)?)?;
    usize::try_from(u64::from_le_bytes(s.try_into().ok()?)).ok()
}

/// The ZIP64 end record referenced by a valid locator just before `eocd`.
fn zip64_end(bytes: &[u8], eocd: usize) -> Option<usize> {
    let locator = eocd.checked_sub(20)?;
    if u32_at(bytes, locator) != Some(0x0706_4B50)
        || u32_at(bytes, add(&[locator, 4])?) != Some(0)
        || u32_at(bytes, add(&[locator, 16])?) != Some(1)
    {
        return None;
    }
    let record = u64_at(bytes, add(&[locator, 8])?)?;
    let size = u64_at(bytes, add(&[record, 4])?)?;
    (u32_at(bytes, record) == Some(0x0606_4B50) && size >= 44 && add(&[record, 12, size]) == Some(locator)).then_some(record)
}

/// `a + b + …` of offsets read from the archive, `None` on overflow.
fn add(parts: &[usize]) -> Option<usize> {
    parts.iter().try_fold(0usize, |acc, x| acc.checked_add(*x))
}

/// Reads every entry of a zip archive (stored or Deflate), within [`MAX_TOTAL`] inflated bytes.
pub fn read(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    read_matching(bytes, |_| true)
}

/// Reads the entries whose names pass `want` (others aren't inflated), within [`MAX_TOTAL`]
/// inflated bytes in all.
pub fn read_matching(bytes: &[u8], want: impl Fn(&str) -> bool) -> Result<Vec<(String, Vec<u8>)>, String> {
    read_matching_within(bytes, want, MAX_TOTAL)
}

/// [`read_matching`] within `budget` inflated bytes in all (at most [`MAX_TOTAL`]).
pub fn read_matching_within(bytes: &[u8], want: impl Fn(&str) -> bool, budget: usize) -> Result<Vec<(String, Vec<u8>)>, String> {
    read_within(bytes, |name| !name.ends_with('/') && want(name), budget, false).map_err(Failure::message)
}

/// Why a read failed.
pub(crate) enum Failure {
    /// The archive's layout can't be followed: not a zip, or damaged. A file of another kind may
    /// merely hold these bytes.
    Layout(String),
    /// Refused whatever precedes the archive: ZIP64 or too many entries (which other readers
    /// open), or a member that is encrypted, too large or damaged.
    Member(String),
    /// The run was asked to stop between members.
    Cancelled,
}

impl Failure {
    fn message(self) -> String {
        match self {
            Failure::Layout(m) | Failure::Member(m) => m,
            Failure::Cancelled => "cancelled".into(),
        }
    }
}

/// Require a single plausible end record before handing the bytes to a different ZIP reader.
/// That reader can fall back to earlier records even after locating a central directory.
fn spreadsheet_end(bytes: &[u8]) -> Result<Option<usize>, Failure> {
    let mut selected = None;
    // Cover the ZIP comment window and zip 9.5.1's in-memory record size allowance.
    let start = bytes.len().saturating_sub((1 << 16) + 64);
    for at in (start..bytes.len().saturating_sub(21)).rev() {
        if u32_at(bytes, at) != Some(0x0605_4B50) {
            continue;
        }
        let comment = u16_at(bytes, at + 20).unwrap_or_default();
        if comment > bytes.len() - at - 22 {
            continue;
        }
        let count = u16_at(bytes, at + 10).unwrap_or_default();
        let size = u32_at(bytes, at + 12).unwrap_or_default();
        let offset = u32_at(bytes, at + 16).unwrap_or_default();
        if size == u32::MAX as usize
            || offset == u32::MAX as usize
            || (count == u16::MAX as usize && at.checked_sub(20).and_then(|p| u32_at(bytes, p)) == Some(0x0706_4B50))
        {
            return Err(Failure::Member("the archive uses ZIP64, which can't be read yet".into()));
        }
        if u32_at(bytes, at + 4) != Some(0) || u16_at(bytes, at + 8) != Some(count) || offset > at || (count != 0 && offset == at) {
            continue;
        }
        if selected.replace(at).is_some() {
            return Err(Failure::Layout("the ZIP archive has ambiguous end records".into()));
        }
    }
    Ok(selected)
}

/// Info-ZIP Unicode Path (APPNOTE 4.6.9). zip 9.5.1 ignores the version byte and
/// applies the last UTF-8 name whose checksum matches the original filename.
fn effective_name<'a>(raw: &'a [u8], mut extra: &'a [u8]) -> Option<&'a [u8]> {
    let mut name = raw;
    let mut raw_crc = None;
    while !extra.is_empty() {
        let kind = u16_at(extra, 0)?;
        let end = 4 + u16_at(extra, 2)?;
        let field = extra.get(4..end)?;
        if kind == 0x7075 {
            let checksum = u32_at(field, 1)?;
            let candidate = field.get(5..)?;
            if checksum == *raw_crc.get_or_insert_with(|| crc32(raw) as usize) && std::str::from_utf8(candidate).is_ok() {
                name = candidate;
            }
        }
        extra = extra.get(end..)?;
    }
    Some(name)
}

/// Every member, including directory payloads (spreadsheet relationships may name any member),
/// within `budget` inflated bytes in all. Cancel is looked at between members; inflating one
/// member, up to [`MAX_ENTRY`], isn't interrupted.
pub(crate) fn read_all_within(bytes: &[u8], budget: usize) -> Result<Vec<(String, Vec<u8>)>, Failure> {
    read_within(bytes, |_| true, budget, true)
}

fn read_within(bytes: &[u8], want: impl Fn(&str) -> bool, budget: usize, spreadsheet: bool) -> Result<Vec<(String, Vec<u8>)>, Failure> {
    let mut budget = budget.min(MAX_TOTAL);
    let bad = || Failure::Layout("not a zip archive (or a damaged one)".to_string());
    let eocd = if spreadsheet {
        spreadsheet_end(bytes)?
    } else {
        // The ZIP end record is in the last 64 KiB + 22 bytes.
        let start = bytes.len().saturating_sub(65_557);
        (start..bytes.len().saturating_sub(21)).rev().find(|&i| u32_at(bytes, i) == Some(0x0605_4B50))
    }
    .ok_or_else(bad)?;
    // ZIP64 puts its 20-byte locator immediately before the ordinary end record.
    // Its count may be a sentinel even when all individual entries fit plain ZIP.
    let zip64_locator = zip64_end(bytes, eocd).is_some();
    if zip64_locator
        || u32_at(bytes, add(&[eocd, 12]).ok_or_else(bad)?) == Some(0xFFFF_FFFF)
        || u32_at(bytes, add(&[eocd, 16]).ok_or_else(bad)?) == Some(0xFFFF_FFFF)
    {
        return Err(Failure::Member("the archive uses ZIP64, which can't be read yet".into()));
    }
    let count = u16_at(bytes, add(&[eocd, 10]).ok_or_else(bad)?).ok_or_else(bad)?;
    if count > MAX_ENTRIES {
        return Err(Failure::Member(format!("the archive holds {count} entries; at most {MAX_ENTRIES} can be read")));
    }
    let recorded = u32_at(bytes, add(&[eocd, 16]).ok_or_else(bad)?).ok_or_else(bad)?;
    let central = if spreadsheet && count != 0 {
        // Like calamine's ZIP reader, allow a preamble not included in recorded offsets.
        let size = u32_at(bytes, add(&[eocd, 12]).ok_or_else(bad)?).ok_or_else(bad)?;
        let end = eocd.saturating_sub(3);
        let is_header = |start| u32_at(bytes, start) == Some(0x0201_4B50);
        eocd.checked_sub(size)
            .filter(|&start| start >= recorded && start < end && is_header(start))
            // A digital-signature trailer may separate the directory from EOCD.
            .or_else(|| (recorded..end).find(|&start| is_header(start)))
            .ok_or_else(bad)?
    } else {
        recorded
    };
    let offset = central.checked_sub(recorded).ok_or_else(bad)?;
    let mut p = central;
    let mut out = Vec::new();
    for _ in 0..count {
        if spreadsheet && crate::progress::check().is_err() {
            return Err(Failure::Cancelled);
        }
        if u32_at(bytes, p) != Some(0x0201_4B50) {
            return Err(bad());
        }
        let at = |off: usize| add(&[p, off]).ok_or_else(bad);
        let method = u16_at(bytes, at(10)?).ok_or_else(bad)?;
        let flags = u16_at(bytes, at(8)?).ok_or_else(bad)?;
        let crc = u32_at(bytes, at(16)?).ok_or_else(bad)?;
        let csize = u32_at(bytes, at(20)?).ok_or_else(bad)?;
        let usize_ = u32_at(bytes, at(24)?).ok_or_else(bad)?;
        let (nl, xl, cl) =
            (u16_at(bytes, at(28)?).ok_or_else(bad)?, u16_at(bytes, at(30)?).ok_or_else(bad)?, u16_at(bytes, at(32)?).ok_or_else(bad)?);
        let local = u32_at(bytes, at(42)?).ok_or_else(bad)?;
        let name_end = add(&[p, 46, nl]).ok_or_else(bad)?;
        let raw = bytes.get(at(46)?..name_end).ok_or_else(bad)?;
        let name = if spreadsheet {
            let extra = bytes.get(name_end..add(&[name_end, xl]).ok_or_else(bad)?).ok_or_else(bad)?;
            effective_name(raw, extra).ok_or_else(bad)?
        } else {
            raw
        };
        let name = String::from_utf8_lossy(name).into_owned();
        p = add(&[p, 46, nl, xl, cl]).ok_or_else(bad)?;
        if !want(&name) {
            continue;
        }
        // Say what's wrong first: an encrypted entry or a ZIP64 one isn't damaged data.
        if flags & 1 != 0 {
            return Err(Failure::Member(format!("{name} is encrypted; its contents can't be read")));
        }
        if csize == 0xFFFF_FFFF || usize_ == 0xFFFF_FFFF || local == 0xFFFF_FFFF {
            return Err(Failure::Member("the archive uses ZIP64, which can't be read yet".into()));
        }
        let local = local.checked_add(offset).ok_or_else(bad)?;
        let lnl = u16_at(bytes, add(&[local, 26]).ok_or_else(bad)?).ok_or_else(bad)?;
        let lxl = u16_at(bytes, add(&[local, 28]).ok_or_else(bad)?).ok_or_else(bad)?;
        let data_at = add(&[local, 30, lnl, lxl]).ok_or_else(bad)?;
        let body = bytes.get(data_at..add(&[data_at, csize]).ok_or_else(bad)?).ok_or_else(bad)?;
        if usize_ > MAX_ENTRY || usize_ > budget {
            // Its expanded size, and the bound it passed: the archive's budget left (which follows
            // the computer's memory where the caller's does) or the most one member may hold.
            let bound = MAX_ENTRY.min(budget);
            return Err(Failure::Member(format!(
                "{name} is too large: it expands to {}, past the {} {}",
                crate::io::size_said(usize_ as u64, true),
                crate::io::size_said(bound as u64, false),
                if budget < MAX_ENTRY { "left to expand from this archive" } else { "a member may expand to" }
            )));
        }
        // Inflate no more than the entry says it holds (and the archive's budget allows).
        let data = match method {
            0 if csize != usize_ => return Err(Failure::Member(format!("{name} is damaged (wrong size)"))),
            0 => body.to_vec(),
            8 => miniz_oxide::inflate::decompress_to_vec_with_limit(body, usize_)
                .map_err(|_| Failure::Member(format!("{name} is damaged")))?,
            m => return Err(Failure::Member(format!("{name} uses an unsupported compression ({m})"))),
        };
        if data.len() != usize_ {
            return Err(Failure::Member(format!("{name} is damaged (wrong size)")));
        }
        budget -= data.len();
        if crc32(&data) as usize != crc {
            return Err(Failure::Member(format!("{name} is damaged (checksum mismatch)")));
        }
        out.push((name, data));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An archive that notes the largest piece offered to it.
    #[derive(Default)]
    struct Pieces {
        archive: Vec<u8>,
        largest: usize,
    }

    impl std::io::Write for Pieces {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.largest = self.largest.max(bytes.len());
            self.archive.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Many small entries make a large central directory: it is handed to the output a record at
    /// a time, so a limit on the output stops it there, and it is never held apart.
    #[test]
    fn the_central_directory_is_written_a_record_at_a_time() {
        let entries: Vec<(String, Vec<u8>)> = (0..500).map(|i| (format!("entry-{i:04}"), vec![b'x'])).collect();
        let mut out = Pieces::default();
        write_to(&entries, &mut out).unwrap();
        // A central record is 46 bytes and the 10 of its name; the directory is 28,000.
        assert_eq!(out.largest, 56);
        assert_eq!(read(&out.archive).unwrap(), entries);
        // Room for every entry and all but the last of the directory is refused.
        let held: usize = entries.iter().map(|(_, bytes)| bytes.capacity()).sum();
        let limit = Limit::new(held + out.archive.len() - 1);
        assert!(write_within(&entries, &limit).is_err() && limit.passed());
    }

    #[test]
    fn spreadsheet_members_use_unicode_path_names() {
        let raw = "legacy.txt";
        let name = "café.txt";
        let mut archive = write(&[(raw.into(), b"hello".to_vec())]).unwrap();
        let end = archive.len() - 22;
        let central = u32_at(&archive, end + 16).unwrap();
        let mut extra = 0x7075u16.to_le_bytes().to_vec();
        extra.extend(((5 + name.len()) as u16).to_le_bytes());
        extra.push(1);
        extra.extend(crc32(raw.as_bytes()).to_le_bytes());
        extra.extend(name.as_bytes());
        archive[central + 30..central + 32].copy_from_slice(&(extra.len() as u16).to_le_bytes());
        let size = (end - central + extra.len()) as u32;
        archive[end + 12..end + 16].copy_from_slice(&size.to_le_bytes());
        archive.splice(end..end, extra);
        assert!(matches!(read_all_within(&archive, 5), Ok(entries) if entries == [(name.into(), b"hello".to_vec())]));
        assert_eq!(read(&archive).unwrap()[0].0, raw);
    }

    #[test]
    fn spreadsheet_end_records_are_checked_before_accepting_an_empty_zip() {
        let empty = write(&[]).unwrap();
        assert!(matches!(read_all_within(&empty, 0), Ok(entries) if entries.is_empty()));
        let archive = write(&[("a.txt".into(), b"hello".to_vec())]).unwrap();
        assert!(matches!(
            read_all_within(&[archive.as_slice(), &empty].concat(), 5),
            Err(Failure::Layout(m)) if m.contains("ambiguous")
        ));
        for bad_field in [8, 20] {
            let mut invalid = empty.clone();
            invalid[bad_field] = 1;
            let archive = [archive.as_slice(), &invalid].concat();
            assert!(matches!(read_all_within(&archive, 5), Ok(entries) if entries[0].1 == b"hello"));
            // Its expanded size and the budget it passed.
            assert!(matches!(
                read_all_within(&archive, 4),
                Err(Failure::Member(m)) if m.contains("too large: it expands to 5 bytes, past the 4 bytes left to expand")
            ));
        }
    }

    #[test]
    fn only_spreadsheet_reads_infer_unrecorded_zip_preambles() {
        let entries = [("a.txt".to_string(), b"hello".to_vec())];
        let archive = write(&entries).unwrap();
        assert_eq!(read(&archive).unwrap(), entries);
        let prefixed = [b"preamble".as_slice(), &archive].concat();
        assert!(read(&prefixed).is_err());
        assert!(matches!(read_all_within(&prefixed, 5), Ok(actual) if actual == entries));
    }

    #[test]
    fn round_trip() {
        let big = "RustGIS ".repeat(1000).into_bytes();
        let entries = vec![("project.json".to_string(), big.clone()), ("data/x.bin".to_string(), vec![1, 2, 3])];
        let z = write(&entries).unwrap();
        assert!(z.len() < big.len(), "compressed");
        let back = read(&z).unwrap_or_default();
        assert_eq!(back, entries);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert!(read(b"not a zip").is_err());
        assert!(read(&z[..z.len() / 2]).is_err());
        // A flipped byte in stored data fails its checksum.
        let mut stored = write(&[("a.bin".to_string(), vec![7u8; 3])]).unwrap();
        if let Some(b) = stored.get_mut(30 + 5) {
            *b ^= 0xFF;
        }
        assert!(read(&stored).is_err());
        // Only the wanted entries are read.
        let only = read_matching(&z, |n| n.starts_with("data/")).unwrap();
        assert_eq!(only.len(), 1);
        // Offsets pointing past the end are refused, not followed.
        let mut far = z.clone();
        let eocd = far.len() - 22;
        far[eocd + 16..eocd + 20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(read(&far).is_err());
    }

    #[test]
    fn encrypted_and_zip64_entries_say_what_they_are() {
        assert_eq!(MAX_ENTRIES, u16::MAX as usize, "plain zip's own entry limit");
        let z = write(&[("a.txt".to_string(), b"hi".to_vec())]).unwrap();
        // The central directory starts where the end-of-central-directory record says, and each
        // entry's header holds the general-purpose flags at +8, sizes at +20/+24 and its local
        // header's offset at +42.
        let central = |z: &[u8]| {
            let e = z.len() - 22;
            u32::from_le_bytes(z[e + 16..e + 20].try_into().unwrap()) as usize
        };
        let mut encrypted = z.clone();
        let fl = central(&encrypted) + 8;
        let f0 = u16::from_le_bytes(encrypted[fl..fl + 2].try_into().unwrap());
        encrypted[fl..fl + 2].copy_from_slice(&(f0 | 1).to_le_bytes());
        assert!(read(&encrypted).err().is_some_and(|e| e.contains("encrypted")));

        let mut zip64 = z.clone();
        let p = central(&zip64);
        zip64[p + 24..p + 28].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(read(&zip64).err().is_some_and(|e| e.contains("ZIP64")));

        let mut offset64 = z.clone();
        let p = central(&offset64);
        offset64[p + 42..p + 46].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(read(&offset64).err().is_some_and(|e| e.contains("ZIP64")));
    }

    #[test]
    fn plain_zip_entry_limit_is_read_but_zip64_count_is_rejected() {
        let entries = vec![("a".to_string(), Vec::new()); MAX_ENTRIES];
        let mut z = write(&entries).unwrap();
        assert_eq!(read(&z).unwrap(), entries);

        // Add one central entry referencing the same empty local entry, taking the
        // count beyond plain ZIP. No entry needs ZIP64 sizes or offsets.
        let eocd = z.len() - 22;
        let central = u32_at(&z, eocd + 16).unwrap();
        let extra = z[central..central + 47].to_vec();
        z.splice(eocd..eocd, extra);
        let zip64_offset = eocd + 47;
        let central_size = (zip64_offset - central) as u64;
        let mut end = Vec::new();
        end.extend_from_slice(&0x0606_4B50u32.to_le_bytes());
        end.extend_from_slice(&44u64.to_le_bytes());
        end.extend_from_slice(&45u16.to_le_bytes());
        end.extend_from_slice(&45u16.to_le_bytes());
        end.extend_from_slice(&[0; 8]); // disk numbers
        end.extend_from_slice(&65_536u64.to_le_bytes());
        end.extend_from_slice(&65_536u64.to_le_bytes());
        end.extend_from_slice(&central_size.to_le_bytes());
        end.extend_from_slice(&(central as u64).to_le_bytes());
        end.extend_from_slice(&0x0706_4B50u32.to_le_bytes());
        end.extend_from_slice(&0u32.to_le_bytes());
        end.extend_from_slice(&(zip64_offset as u64).to_le_bytes());
        end.extend_from_slice(&1u32.to_le_bytes());
        z.splice(zip64_offset..zip64_offset, end);
        let eocd = z.len() - 22;
        z[eocd + 12..eocd + 16].copy_from_slice(&(central_size as u32).to_le_bytes());
        assert!(read(&z).unwrap_err().contains("ZIP64"));
        assert!(read_matching(&z, |_| false).unwrap_err().contains("ZIP64"));
    }

    #[test]
    fn entry_comment_with_zip64_locator_signature_is_plain_zip() {
        let entries = vec![("a".to_string(), Vec::new())];
        let mut z = write(&entries).unwrap();
        let eocd = z.len() - 22;
        let central = u32_at(&z, eocd + 16).unwrap();
        z[central + 32..central + 34].copy_from_slice(&20u16.to_le_bytes());
        let mut comment = vec![0; 20];
        comment[..4].copy_from_slice(&0x0706_4B50u32.to_le_bytes());
        // Even plausible disk fields do not make a comment into a locator.
        comment[16..20].copy_from_slice(&1u32.to_le_bytes());
        z.splice(eocd..eocd, comment);
        let size = (eocd - central + 20) as u32;
        z[eocd + 20 + 12..eocd + 20 + 16].copy_from_slice(&size.to_le_bytes());
        assert_eq!(read(&z).unwrap(), entries);
    }
}
