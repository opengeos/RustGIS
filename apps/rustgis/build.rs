//! On Windows, embeds the app icon in `rustgis.exe`, so Explorer, shortcuts and pinned taskbar
//! entries show the logo. The icon is drawn by the same code as the window icon
//! (`crates/render/src/logo.rs`), so there is no image file to keep in sync.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../crates/render/src/logo.rs");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        windows::embed_icon()?;
    }
    Ok(())
}

#[cfg(windows)]
#[allow(dead_code)]
#[path = "../../crates/render/src/logo.rs"]
mod logo;

#[cfg(windows)]
mod windows {
    use std::path::PathBuf;

    /// Writes an `.ico` of the logo (PNG entries, 16 to 256 px) and a resource script that
    /// references it, then compiles and links the script.
    pub fn embed_icon() -> Result<(), Box<dyn std::error::Error>> {
        let out = PathBuf::from(std::env::var("OUT_DIR")?);
        let ico = out.join("rustgis.ico");
        std::fs::write(&ico, ico_bytes()?)?;
        let version = std::env::var("CARGO_PKG_VERSION")?;
        // VERSIONINFO wants four numbers: drop a pre-release suffix (0.2.0-rc.1 -> 0,2,0,0).
        let nums: Vec<&str> = version.split('-').next().unwrap_or("0.0.0").split('.').collect();
        let num = |i: usize| nums.get(i).copied().unwrap_or("0");
        let file_version = format!("{},{},{},0", num(0), num(1), num(2));
        let rc = out.join("rustgis.rc");
        std::fs::write(
            &rc,
            format!(
                r#"1 ICON "{ico}"
1 VERSIONINFO
FILEVERSION {file_version}
PRODUCTVERSION {file_version}
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "The RustGIS contributors"
      VALUE "FileDescription", "RustGIS"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "rustgis"
      VALUE "LegalCopyright", "Copyright the RustGIS contributors. Apache-2.0."
      VALUE "OriginalFilename", "rustgis.exe"
      VALUE "ProductName", "RustGIS"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
                ico = ico.display().to_string().replace('\\', "\\\\"),
            ),
        )?;
        embed_resource::compile(&rc, embed_resource::NONE).manifest_required()?;
        Ok(())
    }

    /// An ICO file holding the logo as PNG images, which Windows Vista and later read.
    fn ico_bytes() -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        const SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];
        let mut images = Vec::new();
        for size in SIZES {
            let png = super::logo::pixmap(size).ok_or("could not draw the logo")?.encode_png()?;
            images.push((size, png));
        }
        let mut ico = Vec::new();
        ico.extend_from_slice(&[0, 0, 1, 0]);
        ico.extend_from_slice(&u16::try_from(images.len())?.to_le_bytes());
        let mut offset = 6 + 16 * images.len();
        for (size, png) in &images {
            // A dimension of 0 means 256.
            let dim = u8::try_from(*size).unwrap_or(0);
            ico.extend_from_slice(&[dim, dim, 0, 0]);
            ico.extend_from_slice(&1u16.to_le_bytes());
            ico.extend_from_slice(&32u16.to_le_bytes());
            ico.extend_from_slice(&u32::try_from(png.len())?.to_le_bytes());
            ico.extend_from_slice(&u32::try_from(offset)?.to_le_bytes());
            offset += png.len();
        }
        for (_, png) in &images {
            ico.extend_from_slice(png);
        }
        Ok(ico)
    }
}
