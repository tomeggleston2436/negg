// SPDX-License-Identifier: MPL-2.0

//! The scopes of Step 8 of docs/MATHS.md (preview), Ruling 75 (R-scopes,
//! ADR 0022): histogram, RGB parade and vectorscope of an 8-bit sRGB
//! render.
//!
//! The caller renders the whole frame at the first pyramid level whose
//! long side is at most 1024 pixels, applies the frame geometry and the
//! sRGB output transform, and passes the interleaved 8-bit RGB here. Every
//! count is taken on code values; nothing here touches linear light.

use negg_types::{Error, Result};

/// Number of bins of every scope axis: one per 8-bit code value, and the
/// number of parade columns and vectorscope rows and columns (Ruling 75).
pub const SCOPE_BINS: usize = 256;

/// BT.709 luma weights on code values (Ruling 75).
const LUMA: [f64; 3] = [0.2126, 0.7152, 0.0722];

/// BT.709 colour difference divisors: `Cb = (B' - Y') / 1.8556`,
/// `Cr = (R' - Y') / 1.5748` (Ruling 75).
const CB_DIVISOR: f64 = 1.8556;
const CR_DIVISOR: f64 = 1.5748;

/// The scopes of one 8-bit sRGB image (Step 8 of docs/MATHS.md (preview),
/// Ruling 75). Serialised by the session as the `NEGGSCOP` binary layout,
/// in field order.
#[derive(Clone, Debug, PartialEq)]
pub struct Scopes {
    /// Width of the measured image in pixels.
    pub width: u32,
    /// Height of the measured image in pixels.
    pub height: u32,
    /// Counts per code value for R, G, B and luma
    /// `round(0.2126 R + 0.7152 G + 0.0722 B)`, in that order.
    pub histogram: [[u32; SCOPE_BINS]; 4],
    /// RGB parade, `3 * 256 * 256` saturating counts indexed
    /// `[channel][code value][column]`, column `floor(x * 256 / width)`.
    pub parade: Vec<u16>,
    /// Vectorscope, `256 * 256` saturating counts indexed `[row][column]`,
    /// column from Cb and row from Cr (top is positive Cr).
    pub vectorscope: Vec<u16>,
}

/// Luma code value of Ruling 75: `round(0.2126 R + 0.7152 G + 0.0722 B)`
/// on code values, rounded half away from zero, at most 255.
fn luma_code(px: [u8; 3]) -> usize {
    let y = LUMA[0] * f64::from(px[0]) + LUMA[1] * f64::from(px[1]) + LUMA[2] * f64::from(px[2]);
    (y.round() as usize).min(SCOPE_BINS - 1)
}

/// Vectorscope cell `(row, col)` of Ruling 75 for one pixel: BT.709
/// `Cb = (B' - Y') / 1.8556` and `Cr = (R' - Y') / 1.5748` on `code / 255`,
/// then `col = floor((Cb + 0.5) * 256)` and `row = floor((0.5 - Cr) * 256)`,
/// each clamped to `[0, 255]` (the clamp at 0 only absorbs rounding at
/// `Cb = -0.5` or `Cr = 0.5`).
///
/// Because the luma weights sum to 1, `B' - Y'` is computed as
/// `0.2126 (B' - R') + 0.7152 (B' - G')` and `R' - Y'` as
/// `0.7152 (R' - G') + 0.0722 (R' - B')`: algebraically the same, but
/// exactly 0 for a grey, so every grey lands in the centre cell
/// `(128, 128)` instead of rounding into a neighbour.
fn vectorscope_cell(px: [u8; 3]) -> (usize, usize) {
    let [r, g, b] = px.map(|v| f64::from(v) / 255.0);
    let cb = (LUMA[0] * (b - r) + LUMA[1] * (b - g)) / CB_DIVISOR;
    let cr = (LUMA[1] * (r - g) + LUMA[2] * (r - b)) / CR_DIVISOR;
    let bin = |v: f64| ((v * SCOPE_BINS as f64).floor().max(0.0) as usize).min(SCOPE_BINS - 1);
    (bin(0.5 - cr), bin(cb + 0.5))
}

/// Step 8 of docs/MATHS.md (preview), Ruling 75: the histogram, RGB parade
/// and vectorscope of `rgb8`, an interleaved 8-bit sRGB image of `width`
/// by `height` pixels, row-major from the top-left.
///
/// - Histogram: 256 bins for R, G, B and luma
///   `round(0.2126 R + 0.7152 G + 0.0722 B)` on code values.
/// - Parade: per channel, 256 columns (`col = floor(x * 256 / width)`) by
///   256 code values; `u16` counts that saturate at `u16::MAX`.
/// - Vectorscope: BT.709 `Y' = 0.2126 R' + 0.7152 G' + 0.0722 B'`,
///   `Cb = (B' - Y') / 1.8556`, `Cr = (R' - Y') / 1.5748` on `code / 255`;
///   `col = min(255, floor((Cb + 0.5) * 256))`,
///   `row = min(255, floor((0.5 - Cr) * 256))`; `u16` counts that saturate.
///
/// An image with no pixels gives all-zero scopes.
///
/// # Errors
///
/// [`Error::Invalid`] when `rgb8.len()` is not `width * height * 3`, or
/// when `width` or `height` does not fit a `u32`.
pub fn scopes(rgb8: &[u8], width: usize, height: usize) -> Result<Scopes> {
    let expected = width
        .checked_mul(height)
        .and_then(|n| n.checked_mul(3))
        .filter(|&n| n == rgb8.len());
    if expected.is_none() {
        return Err(Error::Invalid(format!(
            "scopes of a {width}x{height} image need {width} * {height} * 3 bytes, got {}",
            rgb8.len()
        )));
    }
    let to_u32 = |v: usize| {
        u32::try_from(v)
            .map_err(|_| Error::Invalid(format!("image dimension {v} does not fit a u32")))
    };
    let mut out = Scopes {
        width: to_u32(width)?,
        height: to_u32(height)?,
        histogram: [[0; SCOPE_BINS]; 4],
        parade: vec![0; 3 * SCOPE_BINS * SCOPE_BINS],
        vectorscope: vec![0; SCOPE_BINS * SCOPE_BINS],
    };
    if width == 0 || height == 0 {
        return Ok(out);
    }
    for row in rgb8.chunks_exact(width * 3) {
        for (x, px) in row.as_chunks::<3>().0.iter().enumerate() {
            let px = *px;
            let col = x * SCOPE_BINS / width;
            for (c, &code) in px.iter().enumerate() {
                let v = usize::from(code);
                out.histogram[c][v] += 1;
                let cell = &mut out.parade[(c * SCOPE_BINS + v) * SCOPE_BINS + col];
                *cell = cell.saturating_add(1);
            }
            out.histogram[3][luma_code(px)] += 1;
            let (vr, vc) = vectorscope_cell(px);
            let cell = &mut out.vectorscope[vr * SCOPE_BINS + vc];
            *cell = cell.saturating_add(1);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: usize, h: usize, px: [u8; 3]) -> Vec<u8> {
        px.repeat(w * h)
    }

    fn parade(s: &Scopes, c: usize, value: usize, col: usize) -> u16 {
        s.parade[(c * SCOPE_BINS + value) * SCOPE_BINS + col]
    }

    #[test]
    fn pure_red_lands_where_bt709_puts_it() {
        let s = scopes(&solid(4, 3, [255, 0, 0]), 4, 3).unwrap();
        assert_eq!((s.width, s.height), (4, 3));
        assert_eq!(s.histogram[0][255], 12);
        assert_eq!(s.histogram[1][0], 12);
        assert_eq!(s.histogram[2][0], 12);
        // Luma round(0.2126 * 255) = round(54.213) = 54.
        assert_eq!(s.histogram[3][54], 12);
        for h in &s.histogram {
            assert_eq!(h.iter().sum::<u32>(), 12);
        }
        // Width 4: x = 0, 1, 2, 3 land in columns 0, 64, 128, 192, three
        // rows each.
        for col in [0, 64, 128, 192] {
            assert_eq!(parade(&s, 0, 255, col), 3);
            assert_eq!(parade(&s, 1, 0, col), 3);
            assert_eq!(parade(&s, 2, 0, col), 3);
        }
        assert_eq!(s.parade.iter().map(|&v| u32::from(v)).sum::<u32>(), 36);
        // Red: Y' = 0.2126, Cb = -0.2126 / 1.8556 = -0.114572...,
        // Cr = 0.7874 / 1.5748 = 0.5. col = floor(0.385428 * 256) = 98,
        // row = floor(0 * 256) = 0.
        assert_eq!(s.vectorscope[98], 12);
        assert_eq!(s.vectorscope.iter().map(|&v| u32::from(v)).sum::<u32>(), 12);
    }

    #[test]
    fn a_grey_ramp_fills_the_diagonal_and_the_centre() {
        // 256 x 1, pixel x has code x in every channel.
        let data: Vec<u8> = (0..=255u8).flat_map(|v| [v, v, v]).collect();
        let s = scopes(&data, 256, 1).unwrap();
        for v in 0..256 {
            for h in &s.histogram {
                assert_eq!(h[v], 1, "code {v}");
            }
            // One column per pixel: the parade is the diagonal.
            for c in 0..3 {
                assert_eq!(parade(&s, c, v, v), 1);
            }
        }
        assert_eq!(s.parade.iter().map(|&v| u32::from(v)).sum::<u32>(), 3 * 256);
        // Greys have Cb = Cr = 0 exactly, so every grey code lands in the
        // centre cell (128, 128) and nowhere else.
        assert_eq!(s.vectorscope[128 * SCOPE_BINS + 128], 256);
        for v in 0..=255u8 {
            assert_eq!(vectorscope_cell([v, v, v]), (128, 128), "grey {v}");
        }
    }

    #[test]
    fn narrow_images_spread_columns_by_floor() {
        // Width 3: col = floor(x * 256 / 3) = 0, 85, 170.
        let s = scopes(&solid(3, 2, [10, 20, 30]), 3, 2).unwrap();
        for col in [0, 85, 170] {
            assert_eq!(parade(&s, 0, 10, col), 2);
            assert_eq!(parade(&s, 1, 20, col), 2);
            assert_eq!(parade(&s, 2, 30, col), 2);
        }
        // Width 512: two pixels per column.
        let s = scopes(&solid(512, 1, [1, 1, 1]), 512, 1).unwrap();
        for col in 0..256 {
            assert_eq!(parade(&s, 0, 1, col), 2);
        }
    }

    #[test]
    fn counts_saturate_at_u16_max() {
        // 1 x 70000: every pixel in parade column 0 and one vectorscope cell.
        let n = 70_000;
        let s = scopes(&solid(1, n, [0, 0, 0]), 1, n).unwrap();
        assert_eq!(parade(&s, 0, 0, 0), u16::MAX);
        let max_cell = *s.vectorscope.iter().max().unwrap();
        assert_eq!(max_cell, u16::MAX);
        // The histogram is u32 and does not saturate.
        assert_eq!(s.histogram[0][0], n as u32);
        assert_eq!(s.histogram[3][0], n as u32);
    }

    #[test]
    fn extremes_stay_inside_the_vectorscope() {
        // Blue has Cb = 0.5 (col clamps to 255); cyan has Cr = -0.5 (row
        // clamps to 255); every primary and secondary lands in range.
        for px in [
            [0, 0, 255],
            [0, 255, 255],
            [255, 255, 0],
            [255, 0, 255],
            [0, 255, 0],
            [255, 0, 0],
        ] {
            let (r, c) = vectorscope_cell(px);
            assert!(r < 256 && c < 256, "{px:?}");
        }
        assert_eq!(vectorscope_cell([255, 0, 0]), (0, 98));
        assert_eq!(vectorscope_cell([0, 0, 255]).1, 255);
        assert_eq!(vectorscope_cell([0, 255, 255]).0, 255);
        assert_eq!(luma_code([255, 255, 255]), 255);
    }

    #[test]
    fn wrong_lengths_are_invalid_and_empty_is_zero() {
        assert!(matches!(scopes(&[0; 11], 2, 2), Err(Error::Invalid(_))));
        assert!(matches!(scopes(&[0; 13], 2, 2), Err(Error::Invalid(_))));
        assert!(matches!(scopes(&[], usize::MAX, 2), Err(Error::Invalid(_))));
        let s = scopes(&[], 0, 0).unwrap();
        assert!(s.histogram.iter().all(|h| h.iter().all(|&v| v == 0)));
        assert_eq!(s.parade.len(), 3 * 256 * 256);
        assert_eq!(s.vectorscope.len(), 256 * 256);
    }
}
