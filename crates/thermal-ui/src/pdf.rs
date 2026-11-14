//! PDF rendering through an embedded pdfium.
//!
//! The pdfium shared library is compiled into the executable (see `build.rs`).
//! On first use it is written to the per-user data directory under a
//! versioned name and loaded from there, so a fresh install renders PDFs with
//! no download and no files beside the exe.
//!
//! pdfium is initialised exactly once per process and only ever used from the
//! render worker thread: creating and destroying `Pdfium` repeatedly (as the
//! first UI did for every preview) re-runs `FPDF_InitLibrary` /
//! `FPDF_DestroyLibrary`, which pdfium does not support reliably.

use image::GrayImage;

use crate::content::{self, LabelPx, PdfPlan, Rotation};

/// The pdfium release the bindings are compiled for (feature `pdfium_7543`).
pub const PDFIUM_RELEASE: &str = "7543";

/// Resolution of the quick pass that finds the content on the page.
const PROBE_DPI: f32 = 72.0;

#[cfg(all(feature = "pdf", pdfium_embedded))]
mod engine {
    use std::path::PathBuf;

    use image::GrayImage;
    use pdfium_render::prelude::{PdfRenderConfig, Pdfium};

    use super::PDFIUM_RELEASE;

    /// The library bytes, compiled in by `build.rs`.
    static LIBRARY: &[u8] = include_bytes!(env!("THERMAL_PDFIUM_LIBRARY"));

    /// A loaded pdfium. Not `Send`: it lives on the render worker thread.
    pub struct PdfEngine {
        pdfium: Pdfium,
    }

    impl PdfEngine {
        /// Writes the embedded library out (once per version) and binds it.
        pub fn load() -> Result<Self, String> {
            let path = unpack()?;
            let bindings = Pdfium::bind_to_library(&path)
                .map_err(|error| format!("load pdfium from {}: {error}", path.display()))?;
            Ok(Self {
                pdfium: Pdfium::new(bindings),
            })
        }

        /// Number of pages.
        pub fn page_count(&self, bytes: &[u8]) -> Result<usize, String> {
            let document = self
                .pdfium
                .load_pdf_from_byte_vec(bytes.to_vec(), None)
                .map_err(describe_open_error)?;
            Ok(usize::from(document.pages().len()))
        }

        /// Page size in points.
        pub fn page_size(&self, bytes: &[u8], page: usize) -> Result<(f32, f32), String> {
            let document = self
                .pdfium
                .load_pdf_from_byte_vec(bytes.to_vec(), None)
                .map_err(describe_open_error)?;
            let count = usize::from(document.pages().len());
            let index =
                u16::try_from(page).map_err(|_| format!("page {} does not exist", page + 1))?;
            let page_ref = document
                .pages()
                .get(index)
                .map_err(|_| format!("page {} does not exist; the PDF has {count}", page + 1))?;
            Ok((page_ref.width().value, page_ref.height().value))
        }

        /// Renders a whole page at `scale` pixels per point, as grey.
        pub fn render(&self, bytes: &[u8], page: usize, scale: f32) -> Result<GrayImage, String> {
            let document = self
                .pdfium
                .load_pdf_from_byte_vec(bytes.to_vec(), None)
                .map_err(describe_open_error)?;
            let index =
                u16::try_from(page).map_err(|_| format!("page {} does not exist", page + 1))?;
            let page_ref = document
                .pages()
                .get(index)
                .map_err(|error| format!("page {}: {error}", page + 1))?;

            let config = PdfRenderConfig::new()
                .scale_page_by_factor(scale)
                // Forms are drawn with their values, as a viewer shows them.
                .render_form_data(true);
            let bitmap = page_ref
                .render_with_config(&config)
                .map_err(|error| format!("render page {}: {error}", page + 1))?;

            let width = u32::try_from(bitmap.width()).unwrap_or(0);
            let height = u32::try_from(bitmap.height()).unwrap_or(0);
            if width == 0 || height == 0 {
                return Err("pdfium produced an empty bitmap".to_string());
            }
            let bytes = bitmap.as_raw_bytes();
            let pixels = width as usize * height as usize;
            if bytes.len() < pixels * 4 {
                return Err(format!(
                    "pdfium returned {} bytes for {width}x{height}",
                    bytes.len()
                ));
            }

            // BGRA, composited on the white clear colour.
            let gray = bytes[..pixels * 4]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|px| {
                    let luma =
                        (114 * u32::from(px[0]) + 587 * u32::from(px[1]) + 299 * u32::from(px[2]))
                            / 1000;
                    u8::try_from(luma).unwrap_or(255)
                })
                .collect();
            GrayImage::from_raw(width, height, gray).ok_or_else(|| "bad page bitmap".to_string())
        }
    }

    fn describe_open_error(error: pdfium_render::prelude::PdfiumError) -> String {
        let text = error.to_string();
        if text.to_ascii_lowercase().contains("password") {
            "the PDF is password protected".to_string()
        } else {
            format!("could not open the PDF ({text})")
        }
    }

    /// Writes the embedded library out, trying the per-user data directory,
    /// then the executable's folder, then the temp folder.
    fn unpack() -> Result<PathBuf, String> {
        let folder = format!("pdfium-{PDFIUM_RELEASE}");
        let candidates = [
            crate::settings::data_dir().map(|dir| dir.join(&folder)),
            std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|dir| dir.join(&folder))),
            Some(std::env::temp_dir().join("thermal-rs").join(&folder)),
        ];
        let mut errors = Vec::new();
        for dir in candidates.into_iter().flatten() {
            match unpack_into(&dir) {
                Ok(path) => return Ok(path),
                Err(error) => errors.push(error),
            }
        }
        Err(format!("could not unpack pdfium: {}", errors.join("; ")))
    }

    fn unpack_into(dir: &std::path::Path) -> Result<PathBuf, String> {
        let name = if cfg!(windows) {
            "pdfium.dll"
        } else if cfg!(target_os = "macos") {
            "libpdfium.dylib"
        } else {
            "libpdfium.so"
        };
        let path = dir.join(name);

        let current = std::fs::metadata(&path)
            .map(|meta| meta.len() == LIBRARY.len() as u64)
            .unwrap_or(false);
        if !current {
            std::fs::create_dir_all(dir)
                .map_err(|error| format!("create {}: {error}", dir.display()))?;
            // Write beside, then rename, so a crash never leaves half a library.
            let partial = dir.join(format!("{name}.partial"));
            std::fs::write(&partial, LIBRARY)
                .map_err(|error| format!("write {}: {error}", partial.display()))?;
            std::fs::rename(&partial, &path)
                .map_err(|error| format!("install {}: {error}", path.display()))?;
        }
        Ok(path)
    }
}

#[cfg(not(all(feature = "pdf", pdfium_embedded)))]
mod engine {
    use image::GrayImage;

    /// Stand-in when PDF support is compiled out.
    pub struct PdfEngine;

    const MISSING: &str = "this build has no PDF support (built with --no-default-features)";

    impl PdfEngine {
        pub fn load() -> Result<Self, String> {
            Err(MISSING.to_string())
        }
        pub fn page_count(&self, _bytes: &[u8]) -> Result<usize, String> {
            Err(MISSING.to_string())
        }
        pub fn page_size(&self, _bytes: &[u8], _page: usize) -> Result<(f32, f32), String> {
            Err(MISSING.to_string())
        }
        pub fn render(
            &self,
            _bytes: &[u8],
            _page: usize,
            _scale: f32,
        ) -> Result<GrayImage, String> {
            Err(MISSING.to_string())
        }
    }
}

pub use engine::PdfEngine;

/// Renders one page fitted to the label (not yet placed on it).
///
/// Two passes: a cheap 72 dpi render finds where the content is, then the page
/// is rendered once at exactly the scale that makes that content fill the
/// label (see [`content::plan_pdf`]), cropped, and turned.
///
/// # Errors
///
/// Returns a user-facing message when pdfium cannot open or render the page.
pub fn render_page(
    engine: &PdfEngine,
    bytes: &[u8],
    page: usize,
    label: LabelPx,
    auto_crop: bool,
    rotation: Rotation,
) -> Result<GrayImage, String> {
    let page_pt = engine.page_size(bytes, page)?;

    let content_box = if auto_crop {
        let probe = engine.render(bytes, page, PROBE_DPI / 72.0)?;
        content::content_fractions(&probe)
    } else {
        None
    };

    let plan: PdfPlan = content::plan_pdf(page_pt, content_box, label, rotation);
    let full = engine.render(bytes, page, plan.scale)?;

    // pdfium rounds the bitmap size its own way; clamp the crop to what we got.
    let (x, y, w, h) = plan.crop;
    let x = x.min(full.width().saturating_sub(1));
    let y = y.min(full.height().saturating_sub(1));
    let w = w.min(full.width() - x).max(1);
    let h = h.min(full.height() - y).max(1);
    let cropped = image::imageops::crop_imm(&full, x, y, w, h).to_image();
    let turned = content::rotate_quarters(&cropped, plan.turns);
    Ok(content::resize_to_fit(turned, label))
}
