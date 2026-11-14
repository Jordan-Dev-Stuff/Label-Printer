//! 8-bit grayscale frame and the halftone step.
//!
//! A [`Frame`] is deliberately dumb: a width, a height and one luminance byte
//! per pixel, row-major. Decoding PNG/JPEG lives in the UI crate so this crate
//! stays dependency-free.

use crate::error::{Error, Result};

/// A grayscale image with 8 bits per pixel, row-major, white = 255.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    width: u32,
    height: u32,
    gray: Vec<u8>,
}

impl Frame {
    /// Creates a white frame.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidFrame`] when either dimension is zero.
    pub fn new(width: u32, height: u32) -> Result<Self> {
        Self::filled(width, height, 255)
    }

    /// Creates a frame filled with one luminance value.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidFrame`] when either dimension is zero.
    pub fn filled(width: u32, height: u32, value: u8) -> Result<Self> {
        if width == 0 || height == 0 {
            return Err(Error::InvalidFrame { width, height });
        }
        let len = width as usize * height as usize;
        Ok(Self {
            width,
            height,
            gray: vec![value; len],
        })
    }

    /// Wraps an existing buffer, checking its length.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidFrame`] when a dimension is zero or the buffer
    /// length is not `width * height`.
    pub fn from_gray(width: u32, height: u32, gray: Vec<u8>) -> Result<Self> {
        if width == 0 || height == 0 || gray.len() != width as usize * height as usize {
            return Err(Error::InvalidFrame { width, height });
        }
        Ok(Self {
            width,
            height,
            gray,
        })
    }

    /// Frame width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Frame height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Pixels, row-major, one byte per pixel.
    #[must_use]
    pub fn gray(&self) -> &[u8] {
        &self.gray
    }

    /// Writes one pixel, ignoring out-of-bounds coordinates.
    pub fn set(&mut self, x: u32, y: u32, value: u8) {
        if x < self.width && y < self.height {
            self.gray[y as usize * self.width as usize + x as usize] = value;
        }
    }

    /// Reads one pixel, returning white for out-of-bounds coordinates.
    #[must_use]
    pub fn get(&self, x: u32, y: u32) -> u8 {
        if x < self.width && y < self.height {
            self.gray[y as usize * self.width as usize + x as usize]
        } else {
            255
        }
    }

    /// Inverts every pixel.
    pub fn invert(&mut self) {
        for value in &mut self.gray {
            *value = 255 - *value;
        }
    }

    /// Binarises with a fixed threshold: `value < threshold` becomes black.
    #[must_use]
    pub fn threshold(&self, threshold: u8) -> Self {
        let gray = self
            .gray
            .iter()
            .map(|&v| if v < threshold { 0 } else { 255 })
            .collect();
        Self {
            width: self.width,
            height: self.height,
            gray,
        }
    }

    /// Binarises with Floyd–Steinberg error diffusion.
    ///
    /// The result contains only 0 (black) and 255 (white), which is what
    /// [`crate::encode`] packs into the TSPL raster.
    #[must_use]
    pub fn dither(&self) -> Self {
        let width = self.width as usize;
        let height = self.height as usize;

        // Work in f32 so diffused error can push values outside 0..=255.
        let mut work: Vec<f32> = self.gray.iter().map(|&v| f32::from(v)).collect();
        let mut out = vec![255_u8; work.len()];

        for y in 0..height {
            for x in 0..width {
                let index = y * width + x;
                let old = work[index];
                let value: u8 = if old < 128.0 { 0 } else { 255 };
                out[index] = value;
                let error = old - f32::from(value);

                let mut distribute = |dx: usize, dy: usize, factor: f32| {
                    let nx = x + dx;
                    let ny = y + dy;
                    if nx < width && ny < height {
                        work[ny * width + nx] += error * factor;
                    }
                };

                distribute(1, 0, 7.0 / 16.0);
                if x > 0 {
                    distribute(0, 1, 3.0 / 16.0);
                }
                distribute(1, 1, 5.0 / 16.0);
                distribute(2, 1, 1.0 / 16.0);
            }
        }

        Self {
            width: self.width,
            height: self.height,
            gray: out,
        }
    }

    /// A checkerboard, useful for checking resolution and media size.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidFrame`] when a dimension is zero.
    pub fn checkerboard(width: u32, height: u32, cell: u32) -> Result<Self> {
        let mut frame = Self::new(width, height)?;
        let cell = cell.max(1);
        for y in 0..height {
            for x in 0..width {
                let dark = ((x / cell) + (y / cell)).is_multiple_of(2);
                frame.set(x, y, if dark { 0 } else { 255 });
            }
        }
        Ok(frame)
    }

    /// A printable test card: border, corner marks, checker strip and a
    /// horizontal gradient. No font is needed, so the core stays dependency
    /// free while still proving orientation, scale and darkness.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidFrame`] when a dimension is zero.
    pub fn test_card(width: u32, height: u32) -> Result<Self> {
        let mut frame = Self::new(width, height)?;
        let margin = 4_u32.min(width / 8).min(height / 8).max(1);
        for x in 0..width {
            for y in 0..height {
                let border =
                    x < margin || y < margin || x >= width - margin || y >= height - margin;
                if border {
                    frame.set(x, y, 0);
                }
            }
        }

        // Corner marks: 40 px squares inset from the frame border.
        let mark = 40_u32.min(width / 4).min(height / 4).max(2);
        let inset = margin + 8;
        for y in inset..(inset + mark).min(height) {
            for x in inset..(inset + mark).min(width) {
                frame.set(x, y, 0);
                frame.set(width.saturating_sub(1 + x), y, 0);
            }
        }

        // Checker strip across the middle.
        let strip_h = (height / 8).max(4);
        let strip_y = height / 2 - strip_h / 2;
        for y in strip_y..(strip_y + strip_h).min(height) {
            for x in 0..width {
                let dark = (x / 8) % 2 == 0;
                frame.set(x, y, if dark { 0 } else { 255 });
            }
        }

        // Horizontal gradient below the strip.
        for y in (strip_y + strip_h).min(height)..height.saturating_sub(margin) {
            for x in 0..width {
                let value = u8::try_from(x.saturating_mul(255) / width.max(1)).unwrap_or(255);
                frame.set(x, y, value);
            }
        }

        Ok(frame)
    }

    /// Box-averaged downscale, for previewing a full label in a small widget.
    ///
    /// Averaging rather than sampling keeps the apparent density of dithered
    /// output, so the thumbnail looks like the printed page at a distance.
    /// Returns a clone when the frame already fits.
    #[must_use]
    pub fn thumbnail(&self, max_width: u32, max_height: u32) -> Self {
        if max_width == 0
            || max_height == 0
            || (self.width <= max_width && self.height <= max_height)
        {
            return self.clone();
        }

        // Integer aspect fit: no float rounding, so a wide frame stays wide and
        // a tall frame stays tall. u64 intermediates cannot overflow for u32
        // dimensions.
        let source_width = u64::from(self.width);
        let source_height = u64::from(self.height);
        let box_width = u64::from(max_width);
        let box_height = u64::from(max_height);

        let (out_width, out_height) = if box_width * source_height <= box_height * source_width {
            // Width-bound: the full requested width fits inside the height box.
            let height = (source_height * box_width / source_width).max(1);
            (max_width, u32::try_from(height).unwrap_or(1))
        } else {
            // Height-bound.
            let width = (source_width * box_height / source_height).max(1);
            (u32::try_from(width).unwrap_or(1), max_height)
        };

        let mut gray = vec![255_u8; out_width as usize * out_height as usize];

        for out_y in 0..out_height {
            let y0 = out_y * self.height / out_height;
            let y1 = ((out_y + 1) * self.height / out_height).max(y0 + 1);

            for out_x in 0..out_width {
                let x0 = out_x * self.width / out_width;
                let x1 = ((out_x + 1) * self.width / out_width).max(x0 + 1);

                let mut sum: u32 = 0;
                let mut count: u32 = 0;
                for y in y0..y1.min(self.height) {
                    for x in x0..x1.min(self.width) {
                        sum += u32::from(self.get(x, y));
                        count += 1;
                    }
                }

                let average = u8::try_from(sum / count.max(1)).unwrap_or(255);
                gray[out_y as usize * out_width as usize + out_x as usize] = average;
            }
        }

        Self {
            width: out_width,
            height: out_height,
            gray,
        }
    }
}
