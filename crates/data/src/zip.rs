//! Zip archives (Deflate or stored): packages bundle a project or map with its data files.

/// Most entries read from an archive.
const MAX_ENTRIES: usize = 10_000;
/// Largest entry inflated (1 GiB).
const MAX_ENTRY: usize = 1 << 30;
/// Most bytes inflated from one archive, all entries together (less in the browser).
pub const MAX_TOTAL: usize = if cfg!(target_arch = "wasm32") { 256 << 20 } else { 1 << 30 };

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, t) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *t = c;
    }
    let mut c = 0xFFFF_FFFFu32;
    for b in data {
        c = table.get(((c ^ *b as u32) & 0xFF) as usize).copied().unwrap_or(0) ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

/// Writes a zip archive of (name, bytes) entries, Deflate-compressed. Plain zip (no Zip64)
/// holds at most 65,535 entries and 4 GiB, and names of up to 65,535 bytes; beyond that it's an
/// error rather than a damaged archive.
pub fn write(entries: &[(String, Vec<u8>)]) -> Result<Vec<u8>, String> {
    let too_big = || "the archive is too large for a zip file".to_string();
    let count = u16::try_from(entries.len()).map_err(|_| too_big())?;
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let n = name.as_bytes();
        let name_len = u16::try_from(n.len()).map_err(|_| too_big())?;
        let data_len = u32::try_from(data.len()).map_err(|_| too_big())?;
        let crc = crc32(data);
        let z = miniz_oxide::deflate::compress_to_vec(data, 6);
        let (method, body): (u16, &[u8]) = if z.len() < data.len() { (8, &z) } else { (0, data) };
        let offset = u32::try_from(out.len()).map_err(|_| too_big())?;
        let body_len = u32::try_from(body.len()).map_err(|_| too_big())?;
        let header = |sig: u32, central: bool| {
            let mut h = Vec::new();
            h.extend_from_slice(&sig.to_le_bytes());
            if central {
                h.extend_from_slice(&20u16.to_le_bytes()); // made by
            }
            h.extend_from_slice(&20u16.to_le_bytes()); // needed to extract
            h.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 names
            h.extend_from_slice(&method.to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes()); // time
            h.extend_from_slice(&0x21u16.to_le_bytes()); // date: 1980-01-01
            h.extend_from_slice(&crc.to_le_bytes());
            h.extend_from_slice(&body_len.to_le_bytes());
            h.extend_from_slice(&data_len.to_le_bytes());
            h.extend_from_slice(&name_len.to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes()); // extra
            if central {
                h.extend_from_slice(&0u16.to_le_bytes()); // comment
                h.extend_from_slice(&0u16.to_le_bytes()); // disk
                h.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
                h.extend_from_slice(&0u32.to_le_bytes()); // external attributes
                h.extend_from_slice(&offset.to_le_bytes());
            }
            h.extend_from_slice(n);
            h
        };
        out.extend_from_slice(&header(0x0403_4B50, false));
        out.extend_from_slice(body);
        central.extend_from_slice(&header(0x0201_4B50, true));
    }
    let cd_offset = u32::try_from(out.len()).map_err(|_| too_big())?;
    let cd_len = u32::try_from(central.len()).map_err(|_| too_big())?;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4B50u32.to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&cd_len.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    Ok(out)
}

fn u16_at(b: &[u8], i: usize) -> Option<usize> {
    let s = b.get(i..i.checked_add(2)?)?;
    Some(u16::from_le_bytes([*s.first()?, *s.get(1)?]) as usize)
}

fn u32_at(b: &[u8], i: usize) -> Option<usize> {
    let s = b.get(i..i.checked_add(4)?)?;
    usize::try_from(u32::from_le_bytes([*s.first()?, *s.get(1)?, *s.get(2)?, *s.get(3)?])).ok()
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
    let mut budget = MAX_TOTAL;
    let bad = || "not a zip archive (or a damaged one)".to_string();
    // The end-of-central-directory record is in the last 64 KiB + 22 bytes.
    let start = bytes.len().saturating_sub(65_557);
    let eocd = (start..bytes.len().saturating_sub(21))
        .rev()
        .find(|&i| bytes.get(i..i.saturating_add(4)) == Some(&[0x50, 0x4B, 0x05, 0x06]))
        .ok_or_else(bad)?;
    let count = u16_at(bytes, add(&[eocd, 10]).ok_or_else(bad)?).ok_or_else(bad)?;
    if count > MAX_ENTRIES {
        return Err(format!("the archive holds {count} entries; at most {MAX_ENTRIES} can be read"));
    }
    let mut p = u32_at(bytes, add(&[eocd, 16]).ok_or_else(bad)?).ok_or_else(bad)?;
    let mut out = Vec::new();
    for _ in 0..count {
        if u32_at(bytes, p) != Some(0x0201_4B50) {
            return Err(bad());
        }
        let at = |off: usize| add(&[p, off]).ok_or_else(bad);
        let method = u16_at(bytes, at(10)?).ok_or_else(bad)?;
        let crc = u32_at(bytes, at(16)?).ok_or_else(bad)?;
        let csize = u32_at(bytes, at(20)?).ok_or_else(bad)?;
        let usize_ = u32_at(bytes, at(24)?).ok_or_else(bad)?;
        let (nl, xl, cl) =
            (u16_at(bytes, at(28)?).ok_or_else(bad)?, u16_at(bytes, at(30)?).ok_or_else(bad)?, u16_at(bytes, at(32)?).ok_or_else(bad)?);
        let local = u32_at(bytes, at(42)?).ok_or_else(bad)?;
        let name = String::from_utf8_lossy(bytes.get(at(46)?..add(&[p, 46, nl]).ok_or_else(bad)?).ok_or_else(bad)?).to_string();
        p = add(&[p, 46, nl, xl, cl]).ok_or_else(bad)?;
        if name.ends_with('/') || !want(&name) {
            continue;
        }
        let lnl = u16_at(bytes, add(&[local, 26]).ok_or_else(bad)?).ok_or_else(bad)?;
        let lxl = u16_at(bytes, add(&[local, 28]).ok_or_else(bad)?).ok_or_else(bad)?;
        let data_at = add(&[local, 30, lnl, lxl]).ok_or_else(bad)?;
        let body = bytes.get(data_at..add(&[data_at, csize]).ok_or_else(bad)?).ok_or_else(bad)?;
        if usize_ > MAX_ENTRY || usize_ > budget {
            return Err(format!("{name} is too large"));
        }
        // Inflate no more than the entry says it holds (and the archive's budget allows).
        let data = match method {
            0 if csize != usize_ => return Err(format!("{name} is damaged (wrong size)")),
            0 => body.to_vec(),
            8 => miniz_oxide::inflate::decompress_to_vec_with_limit(body, usize_).map_err(|_| format!("{name} is damaged"))?,
            m => return Err(format!("{name} uses an unsupported compression ({m})")),
        };
        if data.len() != usize_ {
            return Err(format!("{name} is damaged (wrong size)"));
        }
        budget -= data.len();
        if crc32(&data) as usize != crc {
            return Err(format!("{name} is damaged (checksum mismatch)"));
        }
        out.push((name, data));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
