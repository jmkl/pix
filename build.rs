//! Build script: on Windows, embeds the app icon and version info into the .exe.
//! The icon is generated from `src/icon.rs`, so there is no binary asset.
//! Never fails the build: a missing resource compiler only produces a warning.

#[path = "src/icon.rs"]
#[allow(dead_code)]
mod icon;

use std::io::Write;
use std::path::Path;

/// Write a multi-size .ico (32-bit BMP entries) built from `icon::rgba`.
#[allow(dead_code)]
pub fn write_ico(path: &Path) -> std::io::Result<()> {
    const SIZES: [u32; 4] = [16, 32, 48, 64];

    let mut images: Vec<Vec<u8>> = Vec::new();
    for &s in &SIZES {
        let rgba = icon::rgba(s);
        let mut data = Vec::new();

        // BITMAPINFOHEADER; height is doubled to account for the AND mask.
        data.extend_from_slice(&40u32.to_le_bytes());
        data.extend_from_slice(&(s as i32).to_le_bytes());
        data.extend_from_slice(&((s * 2) as i32).to_le_bytes());
        data.extend_from_slice(&1u16.to_le_bytes()); // planes
        data.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        data.extend_from_slice(&[0u8; 24]); // compression .. important colors

        // Pixels: BGRA, bottom-up.
        for row in (0..s).rev() {
            for col in 0..s {
                let i = ((row * s + col) * 4) as usize;
                data.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
            }
        }
        // AND mask (ignored for 32-bit, but required): all zero.
        let mask_row = (s + 31) / 32 * 4;
        data.extend(std::iter::repeat(0u8).take((mask_row * s) as usize));
        images.push(data);
    }

    let mut ico = Vec::new();
    ico.extend_from_slice(&0u16.to_le_bytes()); // reserved
    ico.extend_from_slice(&1u16.to_le_bytes()); // type: icon
    ico.extend_from_slice(&(SIZES.len() as u16).to_le_bytes());

    let mut offset = 6 + 16 * SIZES.len() as u32;
    for (&s, data) in SIZES.iter().zip(&images) {
        ico.push(s as u8);
        ico.push(s as u8);
        ico.push(0); // palette colors
        ico.push(0); // reserved
        ico.extend_from_slice(&1u16.to_le_bytes()); // planes
        ico.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        ico.extend_from_slice(&(data.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += data.len() as u32;
    }
    for data in &images {
        ico.extend_from_slice(data);
    }

    std::fs::File::create(path)?.write_all(&ico)
}

#[cfg(windows)]
fn embed() {
    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let ico = out_dir.join("pix.ico");
    if let Err(e) = write_ico(&ico) {
        println!("cargo:warning=could not write icon: {e}");
        return;
    }

    let mut res = winresource::WindowsResource::new();
    res.set_icon(ico.to_str().expect("utf-8 path"));
    res.set("ProductName", "Pix");
    res.set("FileDescription", "Pix image viewer");
    if let Err(e) = res.compile() {
        println!("cargo:warning=could not embed the icon (is the Windows SDK installed?): {e}");
    }
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/icon.rs");

    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed();
    }
}
