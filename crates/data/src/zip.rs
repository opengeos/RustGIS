//! Zip archives (Deflate or stored): packages bundle a project or map with its data files.

/// Most entries read from an archive.
const MAX_ENTRIES: usize = 10_000;
/// Largest entry inflated (1 GiB).
const MAX_ENTRY: usize = 1 << 30;

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

/// Writes a zip archive of (name, bytes) entries, Deflate-compressed.
pub fn write(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let crc = crc32(data);
        let z = miniz_oxide::deflate::compress_to_vec(data, 6);
        let (method, body): (u16, &[u8]) = if z.len() < data.len() { (8, &z) } else { (0, data) };
        let offset = out.len() as u32;
        let n = name.as_bytes();
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
            h.extend_from_slice(&(body.len() as u32).to_le_bytes());
            h.extend_from_slice(&(data.len() as u32).to_le_bytes());
            h.extend_from_slice(&(n.len() as u16).to_le_bytes());
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
    let cd_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4B50u32.to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

fn u16_at(b: &[u8], i: usize) -> Option<usize> {
    b.get(i..i + 2).map(|s| u16::from_le_bytes([s[0], s[1]]) as usize)
}

fn u32_at(b: &[u8], i: usize) -> Option<usize> {
    b.get(i..i + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize)
}

/// Reads every entry of a zip archive (stored or Deflate).
pub fn read(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let bad = || "not a zip archive (or a damaged one)".to_string();
    // The end-of-central-directory record is in the last 64 KiB + 22 bytes.
    let start = bytes.len().saturating_sub(65_557);
    let eocd =
        (start..bytes.len().saturating_sub(21)).rev().find(|&i| bytes.get(i..i + 4) == Some(&[0x50, 0x4B, 0x05, 0x06])).ok_or_else(bad)?;
    let count = u16_at(bytes, eocd + 10).ok_or_else(bad)?.min(MAX_ENTRIES);
    let mut p = u32_at(bytes, eocd + 16).ok_or_else(bad)?;
    let mut out = Vec::new();
    for _ in 0..count {
        if u32_at(bytes, p) != Some(0x0201_4B50) {
            return Err(bad());
        }
        let method = u16_at(bytes, p + 10).ok_or_else(bad)?;
        let csize = u32_at(bytes, p + 20).ok_or_else(bad)?;
        let usize_ = u32_at(bytes, p + 24).ok_or_else(bad)?;
        let (nl, xl, cl) =
            (u16_at(bytes, p + 28).ok_or_else(bad)?, u16_at(bytes, p + 30).ok_or_else(bad)?, u16_at(bytes, p + 32).ok_or_else(bad)?);
        let local = u32_at(bytes, p + 42).ok_or_else(bad)?;
        let name = String::from_utf8_lossy(bytes.get(p + 46..p + 46 + nl).ok_or_else(bad)?).to_string();
        p += 46 + nl + xl + cl;
        if name.ends_with('/') {
            continue;
        }
        let (lnl, lxl) = (u16_at(bytes, local + 26).ok_or_else(bad)?, u16_at(bytes, local + 28).ok_or_else(bad)?);
        let data_at = local + 30 + lnl + lxl;
        let body = bytes.get(data_at..data_at.saturating_add(csize)).ok_or_else(bad)?;
        if usize_ > MAX_ENTRY {
            return Err(format!("{name} is too large"));
        }
        let data = match method {
            0 => body.to_vec(),
            8 => miniz_oxide::inflate::decompress_to_vec_with_limit(body, MAX_ENTRY).map_err(|_| format!("{name} is damaged"))?,
            m => return Err(format!("{name} uses an unsupported compression ({m})")),
        };
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
        let z = write(&entries);
        assert!(z.len() < big.len(), "compressed");
        let back = read(&z).unwrap_or_default();
        assert_eq!(back, entries);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert!(read(b"not a zip").is_err());
        assert!(read(&z[..z.len() / 2]).is_err());
    }
}
