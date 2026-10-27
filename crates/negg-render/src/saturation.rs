// SPDX-License-Identifier: MPL-2.0

//! Step 7 of `docs/MATHS.md`: saturation about the Rec.2020 luminance
//! (phase 4, ADR 0033).
//!
//! ```text
//! y     = 0.2627 r + 0.6780 g + 0.0593 b
//! out_c = max(0, y + s * (c - y))
//! ```
//!
//! It runs on the positive after the entry matrix, before the output
//! transform. Saturation 1 skips the step, so the default render is bitwise
//! unchanged.

/// Rec.2020 luminance weights (ITU-R BT.2020 primaries, D65 white).
const LUMA: [f32; 3] = [0.2627, 0.6780, 0.0593];

/// Step 7 of `docs/MATHS.md` (ADR 0033): scales the chroma of one linear
/// pixel by `s` about its Rec.2020 luminance and clips at 0.
///
/// `s == 1.0` (an exact comparison) returns `rgb` unchanged, so the default
/// path is bitwise identical to no saturation step. `s == 0` gives the
/// luminance grey; neutrals and the luminance are preserved for any `s`.
#[inline]
#[must_use]
pub fn saturate_pixel(rgb: [f32; 3], s: f32) -> [f32; 3] {
    if s == 1.0 {
        return rgb;
    }
    let y = LUMA[0] * rgb[0] + LUMA[1] * rgb[1] + LUMA[2] * rgb[2];
    [
        (y + s * (rgb[0] - y)).max(0.0),
        (y + s * (rgb[1] - y)).max(0.0),
        (y + s * (rgb[2] - y)).max(0.0),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saturation_one_is_identity_bitwise() {
        for rgb in [[0.2, 0.5, 0.9], [0.0, 0.0, 0.0], [1.0, 0.3, 0.0]] {
            assert_eq!(saturate_pixel(rgb, 1.0), rgb);
        }
    }

    #[test]
    fn saturation_zero_gives_rec2020_luminance_grey() {
        let rgb = [0.2f32, 0.5, 0.9];
        let y = 0.2627 * rgb[0] + 0.6780 * rgb[1] + 0.0593 * rgb[2];
        for c in saturate_pixel(rgb, 0.0) {
            assert!((c - y).abs() < 1e-6);
        }
    }

    #[test]
    fn saturation_preserves_luminance_and_neutrals() {
        let grey = [0.4f32; 3];
        assert_eq!(saturate_pixel(grey, 1.7), grey);
        let rgb = [0.2f32, 0.5, 0.9];
        let out = saturate_pixel(rgb, 1.5);
        let y = |p: [f32; 3]| 0.2627 * p[0] + 0.6780 * p[1] + 0.0593 * p[2];
        assert!((y(out) - y(rgb)).abs() < 1e-6);
        assert!(out.iter().all(|c| *c >= 0.0));
    }
}
