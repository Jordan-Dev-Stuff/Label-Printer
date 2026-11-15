//! The app icon: a label with a printed barcode on an orange tile. Drawn in
//! code at any size, so the window icon, the header, the Windows `.ico` and
//! the macOS/Linux PNGs all come from the same source and stay sharp.
//!
//! `label-printer --export-icons DIR` writes `label-printer.ico` (committed
//! as `assets/label-printer.ico` and embedded into the Windows exe by
//! `build.rs`) and `label-printer-<size>.png` for the macOS and Linux packages.

use std::path::Path;

/// Sizes packed into the Windows `.ico` (Explorer, taskbar, shortcuts, Alt+Tab).
pub const ICO_SIZES: [u32; 9] = [16, 20, 24, 32, 40, 48, 64, 128, 256];

/// Sizes written as PNGs (macOS `.iconset` and Linux `hicolor` both need them).
pub const PNG_SIZES: [u32; 8] = [16, 32, 48, 64, 128, 256, 512, 1024];

/// The app mark: a label with a printed barcode on an orange tile, drawn at
/// any size so the window icon and header stay sharp.
#[must_use]
pub fn icon_rgba(size: u32) -> Vec<u8> {
    let s = size as f32;
    let mut rgba = vec![0_u8; (size * size * 4) as usize];
    let inside_round = |x: f32, y: f32, x0: f32, y0: f32, x1: f32, y1: f32, r: f32| -> bool {
        if x < x0 || x > x1 || y < y0 || y > y1 {
            return false;
        }
        let cx = x.clamp(x0 + r, x1 - r);
        let cy = y.clamp(y0 + r, y1 - r);
        (x - cx).powi(2) + (y - cy).powi(2) <= r * r
    };
    for py in 0..size {
        for px in 0..size {
            let (x, y) = (px as f32 + 0.5, py as f32 + 0.5);
            let mut color: Option<[u8; 3]> = None;
            if inside_round(x, y, 0.0, 0.0, s, s, s * 0.22) {
                // Orange tile, slightly darker at the bottom.
                let t = y / s;
                let mix = |a: f32, b: f32| (a + (b - a) * t) as u8;
                color = Some([mix(249.0, 194.0), mix(115.0, 65.0), mix(22.0, 12.0)]);
            }
            if inside_round(x, y, s * 0.24, s * 0.16, s * 0.76, s * 0.84, s * 0.06) {
                color = Some([255, 255, 255]);
                // Barcode bars across the middle of the label.
                let bar_zone = y > s * 0.42 && y < s * 0.70 && x > s * 0.31 && x < s * 0.69;
                let bars = [0.0, 0.06, 0.10, 0.18, 0.22, 0.30];
                let pos = (x - s * 0.31) / (s * 0.38);
                let on_bar = bars.iter().any(|&b| pos >= b && pos < b + 0.04)
                    || (pos > 0.40 && pos < 0.48)
                    || (pos > 0.56 && pos < 0.60)
                    || (pos > 0.70 && pos < 0.78)
                    || (pos > 0.86 && pos < 0.90)
                    || pos > 0.95;
                // Text line at the top.
                let text_line = y > s * 0.26 && y < s * 0.32 && x > s * 0.31 && x < s * 0.58;
                if (bar_zone && on_bar) || text_line {
                    color = Some([0x16, 0x17, 0x1B]);
                }
            }
            if let Some([r, g, b]) = color {
                let i = ((py * size + px) * 4) as usize;
                rgba[i..i + 4].copy_from_slice(&[r, g, b, 255]);
            }
        }
    }
    rgba
}

/// Encodes the icon at `size` as PNG.
///
/// # Errors
///
/// Returns a message when PNG encoding fails.
pub fn png(size: u32) -> Result<Vec<u8>, String> {
    let image = image::RgbaImage::from_raw(size, size, icon_rgba(size))
        .ok_or_else(|| "icon buffer has the wrong size".to_string())?;
    let mut out = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut out, image::ImageFormat::Png)
        .map_err(|error| format!("encode {size}px icon: {error}"))?;
    Ok(out.into_inner())
}

/// Packs PNG images into a Windows `.ico` file.
///
/// Layout: a 6-byte header, one 16-byte directory entry per image, then the
/// PNG data. Width and height `0` mean 256 in the directory entry.
#[must_use]
pub fn ico_from_pngs(images: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let count = u16::try_from(images.len()).unwrap_or(u16::MAX);
    let mut out = Vec::new();
    out.extend_from_slice(&0_u16.to_le_bytes()); // reserved
    out.extend_from_slice(&1_u16.to_le_bytes()); // type: icon
    out.extend_from_slice(&count.to_le_bytes());

    let mut offset = 6 + 16 * images.len();
    for (size, data) in images {
        let side = if *size >= 256 {
            0
        } else {
            u8::try_from(*size).unwrap_or(0)
        };
        out.push(side); // width
        out.push(side); // height
        out.push(0); // palette colours
        out.push(0); // reserved
        out.extend_from_slice(&1_u16.to_le_bytes()); // colour planes
        out.extend_from_slice(&32_u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&u32::try_from(data.len()).unwrap_or(u32::MAX).to_le_bytes());
        out.extend_from_slice(&u32::try_from(offset).unwrap_or(u32::MAX).to_le_bytes());
        offset += data.len();
    }
    for (_, data) in images {
        out.extend_from_slice(data);
    }
    out
}

/// Writes `label-printer.ico` and `label-printer-<size>.png` into `dir`.
///
/// # Errors
///
/// Returns a message when encoding or writing fails.
pub fn export(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|error| format!("create {}: {error}", dir.display()))?;

    let mut ico_images = Vec::new();
    for size in ICO_SIZES {
        ico_images.push((size, png(size)?));
    }
    let ico = dir.join("label-printer.ico");
    std::fs::write(&ico, ico_from_pngs(&ico_images))
        .map_err(|error| format!("write {}: {error}", ico.display()))?;

    for size in PNG_SIZES {
        let file = dir.join(format!("label-printer-{size}.png"));
        std::fs::write(&file, png(size)?)
            .map_err(|error| format!("write {}: {error}", file.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_is_opaque_inside_and_transparent_in_the_corners() {
        let size = 64;
        let rgba = icon_rgba(size);
        assert_eq!(rgba.len(), (size * size * 4) as usize);
        let alpha = |x: u32, y: u32| rgba[((y * size + x) * 4 + 3) as usize];
        assert_eq!(alpha(0, 0), 0, "rounded corner is transparent");
        assert_eq!(alpha(32, 32), 255, "centre is opaque");
    }

    #[test]
    fn ico_header_and_directory_point_at_the_images() {
        let images = vec![(16, vec![1, 2, 3]), (256, vec![4, 5])];
        let ico = ico_from_pngs(&images);

        assert_eq!(
            &ico[..6],
            &[0, 0, 1, 0, 2, 0],
            "reserved, type 1, two images"
        );
        // First entry: 16x16, 3 bytes at offset 6 + 2*16 = 38.
        assert_eq!(ico[6], 16);
        assert_eq!(u32::from_le_bytes(ico[14..18].try_into().unwrap()), 3);
        assert_eq!(u32::from_le_bytes(ico[18..22].try_into().unwrap()), 38);
        // Second entry: 256 is written as 0, data right after the first.
        assert_eq!(ico[22], 0);
        assert_eq!(u32::from_le_bytes(ico[34..38].try_into().unwrap()), 41);
        assert_eq!(&ico[38..], &[1, 2, 3, 4, 5]);
    }

    #[test]
    fn png_round_trips() {
        let data = png(32).expect("png");
        let decoded = image::load_from_memory(&data).expect("decode").to_rgba8();
        assert_eq!(decoded.dimensions(), (32, 32));
        assert_eq!(decoded.into_raw(), icon_rgba(32));
    }
}
