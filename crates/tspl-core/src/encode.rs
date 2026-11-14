//! Job geometry, print settings and the TSPL encoder.
//!
//! The byte layout (command order, two-decimal millimetres, `\r\n` line
//! endings, bitmap polarity) is documented in `docs/protocol.md` and pinned by
//! `tests/encode.rs`; a change to any of it is a protocol change.

use crate::error::{Error, Result};
use crate::frame::Frame;

/// Native resolution of the BY-482 family.
pub const DEFAULT_DPI: u32 = 203;

/// Printable width of the BY-482 print head, in millimetres.
pub const MAX_HEAD_WIDTH_MM: f64 = 108.0;

/// Gap between labels on typical 4x6 die-cut stock.
pub const DEFAULT_GAP_MM: f64 = 3.0;

/// Label geometry and print settings for one job.
///
/// A zero `width_mm`/`height_mm` derives the dimension from the frame, so a
/// caller that already scaled the frame can leave both at zero.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Config {
    /// Label width in millimetres; `0.0` derives it from the frame.
    pub width_mm: f64,
    /// Label height in millimetres; `0.0` derives it from the frame.
    pub height_mm: f64,
    /// Gap between labels; `0.0` means continuous media.
    pub gap_mm: f64,
    /// Dots per inch; `0` selects [`DEFAULT_DPI`].
    pub dpi: u32,
    /// Print darkness 1-15; `0` selects 8.
    pub density: u8,
    /// Print speed 1-12; `0` selects 4.
    pub speed: u8,
    /// Copies 1-99; `0` selects 1.
    pub copies: u8,
    /// Send only the bands of rows that contain ink. Smaller jobs, which
    /// matters over Bluetooth; the printed label is identical.
    pub skip_blank: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            width_mm: 0.0,
            height_mm: 0.0,
            gap_mm: DEFAULT_GAP_MM,
            dpi: DEFAULT_DPI,
            density: 8,
            speed: 4,
            copies: 1,
            skip_blank: false,
        }
    }
}

impl Config {
    /// Settings for a PEDOOLO/BEEPRT BY-482: 203 dpi, 108 mm head, 4x6 media.
    #[must_use]
    pub fn by482() -> Self {
        Self::default()
    }

    /// Replaces unset fields with their defaults.
    #[must_use]
    pub fn normalized(mut self) -> Self {
        if self.dpi == 0 {
            self.dpi = DEFAULT_DPI;
        }
        if self.density == 0 {
            self.density = 8;
        }
        if self.speed == 0 {
            self.speed = 4;
        }
        if self.copies == 0 {
            self.copies = 1;
        }
        if self.gap_mm < 0.0 {
            self.gap_mm = 0.0;
        }
        if self.width_mm < 0.0 {
            self.width_mm = 0.0;
        }
        if self.height_mm < 0.0 {
            self.height_mm = 0.0;
        }
        self
    }

    /// Checks every value against the range the printer accepts.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidConfig`] naming the offending field.
    pub fn validate(&self) -> Result<()> {
        if !(50..=1200).contains(&self.dpi) {
            return Err(Error::InvalidConfig(format!(
                "dpi {} out of range 50..=1200",
                self.dpi
            )));
        }
        if !(1..=15).contains(&self.density) {
            return Err(Error::InvalidConfig(format!(
                "density {} out of range 1..=15",
                self.density
            )));
        }
        if !(1..=12).contains(&self.speed) {
            return Err(Error::InvalidConfig(format!(
                "speed {} out of range 1..=12",
                self.speed
            )));
        }
        if !(1..=99).contains(&self.copies) {
            return Err(Error::InvalidConfig(format!(
                "copies {} out of range 1..=99",
                self.copies
            )));
        }
        if self.width_mm > MAX_HEAD_WIDTH_MM {
            return Err(Error::InvalidConfig(format!(
                "label width {:.2} mm exceeds the {:.0} mm head",
                self.width_mm, MAX_HEAD_WIDTH_MM
            )));
        }
        if self.height_mm < 0.0 {
            return Err(Error::InvalidConfig(format!(
                "label height {:.2} mm must not be negative",
                self.height_mm
            )));
        }
        Ok(())
    }
}

/// Converts dots to millimetres at the given resolution.
#[must_use]
pub fn px_to_mm(px: u32, dpi: u32) -> f64 {
    f64::from(px) * 25.4 / f64::from(dpi)
}

/// Blank row runs shorter than this stay inside one `BITMAP` band: a new
/// command costs about 25 bytes, more than a few blank rows.
const BAND_MERGE_ROWS: u32 = 8;

/// Encodes `frame` as a complete TSPL job.
///
/// Rows are packed MSB-first, padded to whole bytes, in the polarity the
/// `BITMAP` command uses: a **cleared bit prints a black dot**, a set bit
/// leaves the paper white. Padding bits are set, so they never print.
///
/// With [`Config::skip_blank`] only the bands of rows that contain ink are
/// sent, each trimmed to its inked bytes; `CLS` already made the rest white.
///
/// # Errors
///
/// Returns [`Error::InvalidConfig`] for out-of-range settings, or when the
/// derived label width exceeds the print head.
pub fn encode(frame: &Frame, config: &Config) -> Result<Vec<u8>> {
    let config = config.normalized();
    config.validate()?;

    let width_px = frame.width();
    let height_px = frame.height();
    let bytes_per_line = width_px.div_ceil(8);

    if u32::from(u16::MAX) < height_px {
        return Err(Error::InvalidConfig(format!(
            "frame height {height_px} exceeds 65535 dot rows"
        )));
    }

    let width_mm = if config.width_mm > 0.0 {
        config.width_mm
    } else {
        px_to_mm(width_px, config.dpi)
    };
    let height_mm = if config.height_mm > 0.0 {
        config.height_mm
    } else {
        px_to_mm(height_px, config.dpi)
    };

    if width_mm > MAX_HEAD_WIDTH_MM {
        return Err(Error::InvalidConfig(format!(
            "label width {width_mm:.2} mm exceeds the {MAX_HEAD_WIDTH_MM:.0} mm head"
        )));
    }

    let mut out = Vec::with_capacity(bytes_per_line as usize * height_px as usize + 256);

    out.extend_from_slice(format!("SIZE {width_mm:.2} mm,{height_mm:.2} mm\r\n").as_bytes());
    out.extend_from_slice(format!("GAP {:.2} mm,0.00 mm\r\n", config.gap_mm).as_bytes());
    out.extend_from_slice(b"DIRECTION 1\r\n");
    out.extend_from_slice(b"REFERENCE 0,0\r\n");
    out.extend_from_slice(format!("DENSITY {}\r\n", config.density).as_bytes());
    out.extend_from_slice(format!("SPEED {}\r\n", config.speed).as_bytes());
    out.extend_from_slice(b"CLS\r\n");

    if config.skip_blank {
        for band in ink_bands(frame) {
            push_bitmap(&mut out, frame, band);
        }
    } else {
        push_bitmap(
            &mut out,
            frame,
            Band {
                first_byte: 0,
                bytes: bytes_per_line,
                top: 0,
                rows: height_px,
            },
        );
    }

    out.extend_from_slice(format!("PRINT {},1\r\n", config.copies).as_bytes());

    Ok(out)
}

/// A rectangle of the raster, in whole bytes horizontally and rows vertically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Band {
    first_byte: u32,
    bytes: u32,
    top: u32,
    rows: u32,
}

/// Appends `BITMAP x,y,width_bytes,rows,0,<data>\r\n` for one band.
fn push_bitmap(out: &mut Vec<u8>, frame: &Frame, band: Band) {
    out.extend_from_slice(
        format!(
            "BITMAP {},{},{},{},0,",
            band.first_byte * 8,
            band.top,
            band.bytes,
            band.rows
        )
        .as_bytes(),
    );

    let width = frame.width() as usize;
    let gray = frame.gray();
    for y in band.top..band.top + band.rows {
        let row = &gray[y as usize * width..(y as usize + 1) * width];
        for byte_index in band.first_byte..band.first_byte + band.bytes {
            let mut packed = 0xFF_u8;
            for bit in 0..8 {
                let x = byte_index as usize * 8 + bit;
                if x < width && row[x] < 128 {
                    packed &= !(0x80 >> bit);
                }
            }
            out.push(packed);
        }
    }
    out.extend_from_slice(b"\r\n");
}

/// Bands of rows containing ink, each trimmed to its inked bytes.
fn ink_bands(frame: &Frame) -> Vec<Band> {
    let width = frame.width() as usize;
    let gray = frame.gray();

    // Inked byte span of each row, or None for a blank row.
    let spans: Vec<Option<(u32, u32)>> = (0..frame.height() as usize)
        .map(|y| {
            let row = &gray[y * width..(y + 1) * width];
            let first = row.iter().position(|&v| v < 128)?;
            let last = row.iter().rposition(|&v| v < 128)?;
            Some((to_u32(first / 8), to_u32(last / 8)))
        })
        .collect();

    let mut bands = Vec::new();
    let mut current: Option<(u32, u32, u32, u32)> = None; // top, bottom, first, last
    let mut blank_run = 0_u32;

    for (y, span) in spans.iter().enumerate() {
        let y = to_u32(y);
        match (*span, current.as_mut()) {
            (Some((first, last)), Some(band)) => {
                band.1 = y;
                band.2 = band.2.min(first);
                band.3 = band.3.max(last);
                blank_run = 0;
            }
            (Some((first, last)), None) => {
                current = Some((y, y, first, last));
                blank_run = 0;
            }
            (None, Some(_)) => {
                blank_run += 1;
                if blank_run >= BAND_MERGE_ROWS {
                    if let Some((top, bottom, first, last)) = current.take() {
                        bands.push(Band {
                            first_byte: first,
                            bytes: last - first + 1,
                            top,
                            rows: bottom - top + 1,
                        });
                    }
                }
            }
            (None, None) => {}
        }
    }
    if let Some((top, bottom, first, last)) = current {
        bands.push(Band {
            first_byte: first,
            bytes: last - first + 1,
            top,
            rows: bottom - top + 1,
        });
    }
    bands
}

/// A job that feeds one label, e.g. to tear off or check alignment.
#[must_use]
pub fn feed_job() -> Vec<u8> {
    b"FORMFEED\r\n".to_vec()
}

/// A job that measures the label and gap with the media sensor.
///
/// `SIZE` and `GAP` come first so the printer knows roughly what to look
/// for; `GAPDETECT` then feeds a few labels and stores the measured values.
///
/// # Errors
///
/// Returns [`Error::InvalidConfig`] for out-of-range settings.
pub fn calibrate_job(config: &Config) -> Result<Vec<u8>> {
    let config = config.normalized();
    config.validate()?;
    if config.width_mm <= 0.0 || config.height_mm <= 0.0 {
        return Err(Error::InvalidConfig(
            "calibration needs the label width and height".to_string(),
        ));
    }

    let mut out = Vec::new();
    out.extend_from_slice(
        format!(
            "SIZE {:.2} mm,{:.2} mm\r\n",
            config.width_mm, config.height_mm
        )
        .as_bytes(),
    );
    out.extend_from_slice(format!("GAP {:.2} mm,0.00 mm\r\n", config.gap_mm).as_bytes());
    out.extend_from_slice(b"GAPDETECT\r\n");
    Ok(out)
}

/// Frame dimensions are `u32`, so indices derived from them always fit.
fn to_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}
