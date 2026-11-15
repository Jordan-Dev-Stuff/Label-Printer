//! Turning content (test card, image, PDF page) into the frame that prints.
//!
//! Everything here is pure raster maths so it is unit-tested without a
//! printer or pdfium. The PDF glue lives in [`crate::pdf`]; it asks
//! [`plan_pdf`] how to render a page so the content lands on the label at the
//! printer's own resolution instead of being rendered small and enlarged.

use image::imageops::FilterType;
use image::GrayImage;
use serde::{Deserialize, Serialize};
use tspl_core::Frame;

/// Luminance below which a pixel counts as content when cropping.
///
/// Slightly below pure white so scanner noise and JPEG ringing around a label
/// edge do not defeat the trim.
pub const CONTENT_THRESHOLD: u8 = 248;

/// Cut-off used by the sharp (non-dithered) halftone. A little above 128 so
/// anti-aliased strokes keep their weight on a 203 dpi head.
pub const SHARP_THRESHOLD: u8 = 150;

/// Largest page bitmap rendered for a PDF, in pixels. A 4x6 label crop out of
/// a Letter page needs ~4 MP; this leaves room for huge pages without letting
/// pdfium allocate gigabytes.
pub const MAX_RENDER_PIXELS: f64 = 60_000_000.0;

/// How content is turned on the label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Rotation {
    /// Turn 90 degrees when the content's orientation fights the label.
    #[default]
    Auto,
    /// As is.
    None,
    /// Quarter turn clockwise.
    Right,
    /// Half turn.
    Upside,
    /// Quarter turn anticlockwise.
    Left,
}

impl Rotation {
    /// All choices, in the order the UI offers them.
    pub const ALL: [Self; 5] = [
        Self::Auto,
        Self::None,
        Self::Right,
        Self::Upside,
        Self::Left,
    ];

    /// Short label for the UI.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::None => "0°",
            Self::Right => "90°",
            Self::Upside => "180°",
            Self::Left => "270°",
        }
    }
}

/// How grey becomes black and white.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Halftone {
    /// Sharp for line art (text, barcodes), dithered for photos.
    #[default]
    Auto,
    /// Fixed threshold: crisp text and scannable barcodes.
    Sharp,
    /// Floyd–Steinberg error diffusion: keeps gradients and photos.
    Photo,
}

impl Halftone {
    /// All choices, in the order the UI offers them.
    pub const ALL: [Self; 3] = [Self::Auto, Self::Sharp, Self::Photo];

    /// Short label for the UI.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Sharp => "Text & barcodes",
            Self::Photo => "Photo",
        }
    }
}

/// Host-side fixes applied before fitting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrepareOptions {
    /// Trim uniform blank margins around the content.
    pub auto_crop: bool,
    /// Turn the content on the label.
    pub rotation: Rotation,
    /// Black-and-white conversion.
    pub halftone: Halftone,
    /// Swap black and white.
    pub invert: bool,
}

impl Default for PrepareOptions {
    fn default() -> Self {
        Self {
            auto_crop: true,
            rotation: Rotation::Auto,
            halftone: Halftone::Auto,
            invert: false,
        }
    }
}

/// Label size in printer dots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LabelPx {
    /// Width in dots.
    pub width: u32,
    /// Height in dots.
    pub height: u32,
}

/// Crops, turns and fits a decoded image to the label (not yet placed).
#[must_use]
pub fn fit_image(source: GrayImage, label: LabelPx, options: PrepareOptions) -> GrayImage {
    let source = if options.auto_crop {
        match content_bounds(&source, CONTENT_THRESHOLD) {
            Some(bounds) => crop_with_margin(&source, bounds),
            // A blank page is left alone; it prints blank either way.
            None => source,
        }
    } else {
        source
    };

    let turns = quarter_turns(
        options.rotation,
        source.width(),
        source.height(),
        label.width,
        label.height,
    );
    let source = rotate_quarters(&source, turns);
    resize_to_fit(source, label)
}

/// Scales `image` to the largest size that fits the label, keeping its aspect.
///
/// Images within two dots of the target are left untouched: resampling a
/// barcode by 0.2% only blurs its bars.
#[must_use]
pub fn resize_to_fit(image: GrayImage, label: LabelPx) -> GrayImage {
    let (target_w, target_h) =
        fit_dimensions(image.width(), image.height(), label.width, label.height);
    let close = image.width().abs_diff(target_w) <= 2
        && image.height().abs_diff(target_h) <= 2
        && image.width() <= label.width
        && image.height() <= label.height;
    if close {
        return image;
    }
    let filter = if target_w < image.width() {
        // Downscaling: an area-style filter keeps thin strokes.
        FilterType::Triangle
    } else {
        FilterType::CatmullRom
    };
    image::imageops::resize(&image, target_w, target_h, filter)
}

/// Centres `image` on a white label-sized frame, clipping anything larger.
///
/// # Errors
///
/// Returns a message when the label frame cannot be allocated.
pub fn place_on_label(image: &GrayImage, label: LabelPx) -> Result<Frame, String> {
    let mut frame = Frame::new(label.width, label.height).map_err(|error| error.to_string())?;
    let offset_x = label.width.saturating_sub(image.width()) / 2;
    let offset_y = label.height.saturating_sub(image.height()) / 2;
    for (x, y, pixel) in image.enumerate_pixels() {
        let (fx, fy) = (offset_x + x, offset_y + y);
        if fx < label.width && fy < label.height {
            frame.set(fx, fy, pixel.0[0]);
        }
    }
    Ok(frame)
}

/// Converts a grey frame to pure black and white as the options ask.
#[must_use]
pub fn binarize(frame: &Frame, options: PrepareOptions) -> Frame {
    let sharp = match options.halftone {
        Halftone::Sharp => true,
        Halftone::Photo => false,
        Halftone::Auto => is_line_art(frame.gray()),
    };
    let mut out = if sharp {
        frame.threshold(SHARP_THRESHOLD)
    } else {
        frame.dither()
    };
    if options.invert {
        out.invert();
    }
    out
}

/// True when the content is mostly black and white with few mid-tones.
///
/// Rendered text and barcodes are bimodal: anti-aliasing only greys their
/// edges. Photos and gradients are not. Dithering line art makes bar edges
/// ragged and barcodes harder to scan, so the auto halftone thresholds it.
#[must_use]
pub fn is_line_art(gray: &[u8]) -> bool {
    if gray.is_empty() {
        return true;
    }
    let mid = gray.iter().filter(|&&v| (48..=208).contains(&v)).count();
    // Integer maths: mid / total < 8%.
    mid * 100 < gray.len() * 8
}

/// Bounding box of non-white pixels, inclusive, as `(left, top, right, bottom)`.
///
/// Returns `None` when the image is entirely blank.
#[must_use]
pub fn content_bounds(image: &GrayImage, threshold: u8) -> Option<(u32, u32, u32, u32)> {
    let (width, height) = image.dimensions();
    let mut left = width;
    let mut top = height;
    let mut right = 0;
    let mut bottom = 0;
    let mut found = false;

    for (x, y, pixel) in image.enumerate_pixels() {
        if pixel.0[0] < threshold {
            found = true;
            left = left.min(x);
            top = top.min(y);
            right = right.max(x);
            bottom = bottom.max(y);
        }
    }

    found.then_some((left, top, right, bottom))
}

/// Margin kept around cropped content: 1% of its larger side, at least 2 px.
#[must_use]
pub fn crop_margin(bounds: (u32, u32, u32, u32)) -> u32 {
    let (left, top, right, bottom) = bounds;
    ((right - left).max(bottom - top) / 100).max(2)
}

/// Crops to `bounds` plus a small margin so content never touches the edge.
#[must_use]
pub fn crop_with_margin(image: &GrayImage, bounds: (u32, u32, u32, u32)) -> GrayImage {
    let (width, height) = image.dimensions();
    let (left, top, right, bottom) = bounds;
    let margin = crop_margin(bounds);

    let crop_left = left.saturating_sub(margin);
    let crop_top = top.saturating_sub(margin);
    let crop_right = (right + margin).min(width.saturating_sub(1));
    let crop_bottom = (bottom + margin).min(height.saturating_sub(1));

    image::imageops::crop_imm(
        image,
        crop_left,
        crop_top,
        crop_right - crop_left + 1,
        crop_bottom - crop_top + 1,
    )
    .to_image()
}

/// Rotates clockwise by `turns` quarter turns.
#[must_use]
pub fn rotate_quarters(image: &GrayImage, turns: u8) -> GrayImage {
    match turns % 4 {
        1 => image::imageops::rotate90(image),
        2 => image::imageops::rotate180(image),
        3 => image::imageops::rotate270(image),
        _ => image.clone(),
    }
}

/// Quarter turns clockwise for a rotation choice and content/label shapes.
#[must_use]
pub fn quarter_turns(
    rotation: Rotation,
    content_w: u32,
    content_h: u32,
    label_w: u32,
    label_h: u32,
) -> u8 {
    match rotation {
        Rotation::Auto => u8::from(should_rotate(content_w, content_h, label_w, label_h)),
        Rotation::None => 0,
        Rotation::Right => 1,
        Rotation::Upside => 2,
        Rotation::Left => 3,
    }
}

/// True when rotating the content would align it with the label.
///
/// Square-ish content within 5% is never rotated: it fits either way.
#[must_use]
pub fn should_rotate(content_w: u32, content_h: u32, label_w: u32, label_h: u32) -> bool {
    if content_w == 0 || content_h == 0 || label_w == 0 || label_h == 0 {
        return false;
    }

    let ratio = f64::from(content_w) / f64::from(content_h);
    if (0.95..=1.05).contains(&ratio) {
        return false;
    }

    (content_w > content_h) != (label_w > label_h)
}

/// Largest size that fits `source` inside `box_w` x `box_h`, preserving aspect.
#[must_use]
pub fn fit_dimensions(source_w: u32, source_h: u32, box_w: u32, box_h: u32) -> (u32, u32) {
    if source_w == 0 || source_h == 0 || box_w == 0 || box_h == 0 {
        return (box_w.max(1), box_h.max(1));
    }

    let scale = f64::min(
        f64::from(box_w) / f64::from(source_w),
        f64::from(box_h) / f64::from(source_h),
    );

    let width = to_px(f64::from(source_w) * scale);
    let height = to_px(f64::from(source_h) * scale);
    (width.min(box_w.max(1)), height.min(box_h.max(1)))
}

/// How to render one PDF page so its content fills the label at full resolution.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PdfPlan {
    /// Pixels per PDF point for the final render.
    pub scale: f32,
    /// Bitmap size of the whole page at `scale`.
    pub page_px: (u32, u32),
    /// Region of that bitmap to keep: `(x, y, width, height)`.
    pub crop: (u32, u32, u32, u32),
    /// Quarter turns clockwise applied after cropping.
    pub turns: u8,
}

/// Plans the final PDF render.
///
/// `content` is the content's bounding box as fractions of the page
/// (`left, top, right, bottom` in 0..=1), from a quick low-resolution pass, or
/// `None` to use the whole page. The page is then rendered once at exactly the
/// scale that makes that region fill the label, so a 4x6 shipping label in the
/// corner of a Letter page prints at 203 dpi rather than being rendered at
/// label width and enlarged twofold.
#[must_use]
pub fn plan_pdf(
    page_pt: (f32, f32),
    content: Option<(f32, f32, f32, f32)>,
    label: LabelPx,
    rotation: Rotation,
) -> PdfPlan {
    let (page_w, page_h) = (page_pt.0.max(1.0), page_pt.1.max(1.0));
    let (left, top, right, bottom) = content
        .map(|(l, t, r, b)| {
            let clamp = |v: f32| v.clamp(0.0, 1.0);
            let (l, r) = (clamp(l.min(r)), clamp(l.max(r)));
            let (t, b) = (clamp(t.min(b)), clamp(t.max(b)));
            if r - l <= 0.0 || b - t <= 0.0 {
                (0.0, 0.0, 1.0, 1.0)
            } else {
                (l, t, r, b)
            }
        })
        .unwrap_or((0.0, 0.0, 1.0, 1.0));

    let region_w = (right - left) * page_w;
    let region_h = (bottom - top) * page_h;

    let turns = quarter_turns(
        rotation,
        to_px(f64::from(region_w)),
        to_px(f64::from(region_h)),
        label.width,
        label.height,
    );
    let (turned_w, turned_h) = if turns % 2 == 1 {
        (region_h, region_w)
    } else {
        (region_w, region_h)
    };

    let mut scale = f32::min(
        label.width as f32 / turned_w.max(0.01),
        label.height as f32 / turned_h.max(0.01),
    );

    // Never let a tiny crop of a huge page ask for an absurd bitmap.
    let pixels = f64::from(page_w * scale) * f64::from(page_h * scale);
    if pixels > MAX_RENDER_PIXELS {
        scale *= (MAX_RENDER_PIXELS / pixels).sqrt() as f32;
    }

    let page_px = (
        to_px(f64::from(page_w * scale)),
        to_px(f64::from(page_h * scale)),
    );
    let x = to_px_floor(f64::from(left * page_w * scale)).min(page_px.0 - 1);
    let y = to_px_floor(f64::from(top * page_h * scale)).min(page_px.1 - 1);
    let w = to_px(f64::from(region_w * scale)).min(page_px.0 - x);
    let h = to_px(f64::from(region_h * scale)).min(page_px.1 - y);

    PdfPlan {
        scale,
        page_px,
        crop: (x, y, w.max(1), h.max(1)),
        turns,
    }
}

/// Content bounds of a low-resolution page render as page fractions,
/// including the same margin [`crop_with_margin`] keeps.
#[must_use]
pub fn content_fractions(preview: &GrayImage) -> Option<(f32, f32, f32, f32)> {
    let bounds = content_bounds(preview, CONTENT_THRESHOLD)?;
    let margin = crop_margin(bounds);
    let (w, h) = (preview.width() as f32, preview.height() as f32);
    let (left, top, right, bottom) = bounds;
    Some((
        left.saturating_sub(margin) as f32 / w,
        top.saturating_sub(margin) as f32 / h,
        ((right + margin + 1) as f32 / w).min(1.0),
        ((bottom + margin + 1) as f32 / h).min(1.0),
    ))
}

/// Rounds a non-negative float to whole pixels, at least one.
fn to_px(value: f64) -> u32 {
    if value.is_finite() && value >= 1.0 {
        value.round().min(f64::from(u32::MAX)) as u32
    } else {
        1
    }
}

/// Floors a non-negative float to whole pixels.
fn to_px_floor(value: f64) -> u32 {
    if value.is_finite() && value > 0.0 {
        value.floor().min(f64::from(u32::MAX)) as u32
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Luma;

    const LABEL_4X6: LabelPx = LabelPx {
        width: 812,
        height: 1218,
    };

    /// 200x100 white page with a black 40x20 block at (80, 40).
    fn page_with_block() -> GrayImage {
        let mut image = GrayImage::from_pixel(200, 100, Luma([255]));
        for y in 40..60 {
            for x in 80..120 {
                image.put_pixel(x, y, Luma([0]));
            }
        }
        image
    }

    #[test]
    fn bounds_find_the_content() {
        let bounds = content_bounds(&page_with_block(), CONTENT_THRESHOLD).expect("content");
        assert_eq!(bounds, (80, 40, 119, 59));
    }

    #[test]
    fn bounds_are_none_for_a_blank_page() {
        let blank = GrayImage::from_pixel(10, 10, Luma([255]));
        assert!(content_bounds(&blank, CONTENT_THRESHOLD).is_none());
        assert!(content_fractions(&blank).is_none());
    }

    #[test]
    fn crop_trims_blank_margins_but_keeps_a_border() {
        let image = page_with_block();
        let bounds = content_bounds(&image, CONTENT_THRESHOLD).expect("content");
        let cropped = crop_with_margin(&image, bounds);

        assert_eq!((cropped.width(), cropped.height()), (44, 24));
        assert_eq!(cropped.get_pixel(2, 2).0[0], 0);
        assert_eq!(cropped.get_pixel(0, 0).0[0], 255);
    }

    #[test]
    fn crop_at_the_edge_does_not_overflow() {
        let mut image = GrayImage::from_pixel(10, 10, Luma([255]));
        image.put_pixel(0, 0, Luma([0]));
        image.put_pixel(9, 9, Luma([0]));
        let bounds = content_bounds(&image, CONTENT_THRESHOLD).expect("content");
        let cropped = crop_with_margin(&image, bounds);
        assert_eq!(cropped.dimensions(), (10, 10));
    }

    #[test]
    fn rotation_is_clockwise() {
        let mut image = GrayImage::from_pixel(3, 2, Luma([255]));
        image.put_pixel(0, 0, Luma([0]));

        let rotated = rotate_quarters(&image, 1);
        assert_eq!((rotated.width(), rotated.height()), (2, 3));
        assert_eq!(rotated.get_pixel(1, 0).0[0], 0);
        assert_eq!(rotated.get_pixel(0, 0).0[0], 255);

        // Four quarter turns are the identity; two flip the corner.
        assert_eq!(rotate_quarters(&image, 4), image);
        let half = rotate_quarters(&image, 2);
        assert_eq!(half.get_pixel(2, 1).0[0], 0);
    }

    #[test]
    fn quarter_turns_follow_the_choice() {
        let cases = [
            (Rotation::Auto, 600, 300, 1),
            (Rotation::Auto, 300, 600, 0),
            (Rotation::None, 600, 300, 0),
            (Rotation::Right, 300, 600, 1),
            (Rotation::Upside, 300, 600, 2),
            (Rotation::Left, 300, 600, 3),
        ];
        for (rotation, w, h, want) in cases {
            assert_eq!(
                quarter_turns(rotation, w, h, 812, 1218),
                want,
                "{rotation:?} {w}x{h}"
            );
        }
    }

    #[test]
    fn rotation_follows_the_label_orientation() {
        assert!(should_rotate(600, 300, 812, 1218));
        assert!(!should_rotate(300, 600, 812, 1218));
        assert!(!should_rotate(600, 300, 1218, 812));
        assert!(!should_rotate(500, 505, 812, 1218));
        assert!(!should_rotate(0, 0, 812, 1218));
    }

    #[test]
    fn fit_preserves_aspect_ratio() {
        assert_eq!(fit_dimensions(812, 1218, 812, 1218), (812, 1218));
        assert_eq!(fit_dimensions(2550, 3300, 812, 1218), (812, 1051));
        assert_eq!(fit_dimensions(1000, 100, 812, 1218), (812, 81));
        assert_eq!(fit_dimensions(100, 1000, 812, 1218), (122, 1218));
    }

    #[test]
    fn fit_handles_degenerate_input() {
        assert_eq!(fit_dimensions(0, 10, 100, 100), (100, 100));
        assert_eq!(fit_dimensions(10, 0, 100, 100), (100, 100));
        assert_eq!(fit_dimensions(10, 10, 0, 0), (1, 1));
    }

    #[test]
    fn fit_never_exceeds_the_box() {
        for (sw, sh) in [(1, 1), (9000, 10), (10, 9000), (2048, 2048)] {
            let (w, h) = fit_dimensions(sw, sh, 812, 1218);
            assert!(w <= 812 && h <= 1218, "{sw}x{sh} -> {w}x{h}");
            assert!(w >= 1 && h >= 1, "{sw}x{sh} -> {w}x{h}");
        }
    }

    #[test]
    fn near_exact_sizes_are_not_resampled() {
        let mut image = GrayImage::from_pixel(811, 1217, Luma([255]));
        image.put_pixel(5, 5, Luma([0]));
        let out = resize_to_fit(image.clone(), LABEL_4X6);
        assert_eq!(out, image, "a 1-dot difference must not blur the content");

        let big = GrayImage::from_pixel(1624, 2436, Luma([255]));
        assert_eq!(resize_to_fit(big, LABEL_4X6).dimensions(), (812, 1218));
    }

    #[test]
    fn placing_centres_and_clips() {
        let image = GrayImage::from_pixel(2, 2, Luma([0]));
        let frame = place_on_label(
            &image,
            LabelPx {
                width: 6,
                height: 4,
            },
        )
        .expect("frame");
        assert_eq!(frame.get(2, 1), 0);
        assert_eq!(frame.get(0, 0), 255);

        let huge = GrayImage::from_pixel(50, 50, Luma([0]));
        let frame = place_on_label(
            &huge,
            LabelPx {
                width: 6,
                height: 4,
            },
        )
        .expect("frame");
        assert!(frame.gray().iter().all(|&v| v == 0));
    }

    #[test]
    fn line_art_detection() {
        let mut text = vec![255_u8; 1000];
        text[..100].fill(0);
        text[100..130].fill(128); // anti-aliased edges
        assert!(is_line_art(&text));

        let gradient: Vec<u8> = (0..1000)
            .map(|i| u8::try_from(i * 255 / 999).unwrap())
            .collect();
        assert!(!is_line_art(&gradient));
        assert!(is_line_art(&[]));
    }

    #[test]
    fn binarize_respects_halftone_and_invert() {
        let gray: Vec<u8> = (0..64).map(|i| u8::try_from(i * 4).unwrap()).collect();
        let frame = Frame::from_gray(8, 8, gray).expect("frame");

        let sharp = binarize(
            &frame,
            PrepareOptions {
                halftone: Halftone::Sharp,
                ..PrepareOptions::default()
            },
        );
        let blacks = sharp.gray().iter().filter(|&&v| v == 0).count();
        assert_eq!(blacks, usize::from(SHARP_THRESHOLD).div_ceil(4));

        let inverted = binarize(
            &frame,
            PrepareOptions {
                halftone: Halftone::Sharp,
                invert: true,
                ..PrepareOptions::default()
            },
        );
        assert_eq!(
            inverted.gray().iter().filter(|&&v| v == 0).count(),
            64 - blacks
        );

        let photo = binarize(
            &frame,
            PrepareOptions {
                halftone: Halftone::Photo,
                ..PrepareOptions::default()
            },
        );
        assert!(photo.gray().iter().all(|&v| v == 0 || v == 255));
    }

    #[test]
    fn fit_image_uses_the_full_label_and_centres_content() {
        let fitted = fit_image(
            page_with_block(),
            LabelPx {
                width: 400,
                height: 200,
            },
            PrepareOptions::default(),
        );
        let frame = place_on_label(
            &fitted,
            LabelPx {
                width: 400,
                height: 200,
            },
        )
        .expect("frame");
        assert_eq!((frame.width(), frame.height()), (400, 200));
        assert!(frame.gray().iter().any(|&v| v < 128), "content survived");
    }

    #[test]
    fn pdf_plan_renders_a_shipping_label_corner_at_full_resolution() {
        // Letter page (612x792 pt) with a 4x6 in label (288x432 pt) in the
        // top-left corner: the plan must render the page at 203 dpi exactly.
        let content = Some((0.0, 0.0, 288.0 / 612.0, 432.0 / 792.0));
        let plan = plan_pdf((612.0, 792.0), content, LABEL_4X6, Rotation::Auto);

        let dpi = plan.scale * 72.0;
        assert!((dpi - 203.0).abs() < 0.5, "rendered at {dpi} dpi");
        assert_eq!(plan.turns, 0);
        let (_, _, w, h) = plan.crop;
        assert!(w.abs_diff(812) <= 1 && h.abs_diff(1218) <= 1, "{w}x{h}");
    }

    #[test]
    fn pdf_plan_turns_landscape_content_and_swaps_the_fit() {
        // A 6x4 landscape page on a 4x6 portrait label.
        let plan = plan_pdf((432.0, 288.0), None, LABEL_4X6, Rotation::Auto);
        assert_eq!(plan.turns, 1);
        assert_eq!(plan.page_px, (1218, 812));
        assert_eq!(plan.crop, (0, 0, 1218, 812));

        let plan = plan_pdf((432.0, 288.0), None, LABEL_4X6, Rotation::None);
        assert_eq!(plan.turns, 0);
        assert_eq!(plan.crop.2, 812, "unturned landscape is width-bound");
    }

    #[test]
    fn pdf_plan_survives_degenerate_input() {
        for content in [
            None,
            Some((0.5, 0.5, 0.5, 0.5)),
            Some((2.0, -1.0, -3.0, 9.0)),
        ] {
            let plan = plan_pdf((0.0, 0.0), content, LABEL_4X6, Rotation::Auto);
            assert!(plan.scale.is_finite() && plan.scale > 0.0, "{content:?}");
            let (x, y, w, h) = plan.crop;
            assert!(
                x + w <= plan.page_px.0 && y + h <= plan.page_px.1,
                "{plan:?}"
            );
        }
    }

    #[test]
    fn pdf_plan_caps_huge_renders() {
        // A 1-point speck on a 200x200 inch poster would need a gigantic bitmap.
        let content = Some((0.5, 0.5, 0.5001, 0.5001));
        let plan = plan_pdf((14_400.0, 14_400.0), content, LABEL_4X6, Rotation::Auto);
        let pixels = f64::from(plan.page_px.0) * f64::from(plan.page_px.1);
        assert!(pixels <= MAX_RENDER_PIXELS * 1.01, "{pixels}");
    }

    #[test]
    fn content_fractions_include_the_margin() {
        let (l, t, r, b) = content_fractions(&page_with_block()).expect("content");
        // Block spans x 80..=119, y 40..=59, margin 2.
        assert!((l - 78.0 / 200.0).abs() < 1e-6);
        assert!((t - 38.0 / 100.0).abs() < 1e-6);
        assert!((r - 122.0 / 200.0).abs() < 1e-6);
        assert!((b - 62.0 / 100.0).abs() < 1e-6);
    }
}
