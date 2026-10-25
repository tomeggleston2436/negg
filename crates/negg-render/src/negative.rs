// SPDX-License-Identifier: MPL-2.0

//! Step 8 of `docs/MATHS.md` (preview, "The negative before analysis",
//! Ruling 134): the decoded linear negative made viewable, with no
//! inversion, so that the app can show a frame (and let the user draw its
//! analysis area) before pass 1 has run.
//!
//! The display is a single exposure scale for all three channels, so the
//! film's mask colour and the holder stay as they are: every sample is
//! multiplied by [`negative_display_scale`] (which maps the
//! [`NEGATIVE_WHITE_PERCENTILE`]th percentile of the frame's block medians
//! to 1), passed through the step 7 entry matrix when the caller has one,
//! and clipped to `[0, 1]`. The caller turns the result to sRGB with its
//! display transform (`negg-cms`). Nothing here is used by the export.

use negg_film::{AnalysisGrid, percentile};
use negg_types::Image;

use crate::matrix::apply_matrix_pixel;
use crate::par::for_each_pixel_mut;

/// The percentile of the block medians (all channels together) that the
/// negative display maps to white. Implementer choice for a preview only,
/// TODO(measure).
pub const NEGATIVE_WHITE_PERCENTILE: f32 = 99.5;

/// Step 8 of `docs/MATHS.md` (`docs/MATHS.md#step-8-export`, preview, the
/// negative before analysis): the exposure scale of the negative display,
/// `1 / P` with `P` the [`NEGATIVE_WHITE_PERCENTILE`]th percentile of every
/// sample of the analysis grid's block medians (the three channels pooled).
/// `1` when the grid is empty or `P` is not finite and positive.
///
/// Taken from the analysis grid so that the thumbnail and every tile of
/// every pyramid level share one scale.
pub fn negative_display_scale(grid: &AnalysisGrid) -> f32 {
    let mut samples: Vec<f32> = grid
        .median
        .data()
        .iter()
        .copied()
        .filter(|v| v.is_finite())
        .collect();
    if samples.is_empty() {
        return 1.0;
    }
    let white = percentile(&mut samples, NEGATIVE_WHITE_PERCENTILE);
    if white.is_finite() && white > 0.0 {
        1.0 / white
    } else {
        1.0
    }
}

/// Step 8 of `docs/MATHS.md` (`docs/MATHS.md#step-8-export`, preview, the
/// negative before analysis): one pixel of the negative display, `clip(M
/// (scale * v), 0, 1)` per channel, with `M` the step 7 entry matrix when
/// given (no matrix when `None`). Not inverted. A non-finite sample shows
/// as 0.
#[inline]
#[must_use]
pub fn negative_display_pixel(
    v: [f32; 3],
    scale: f32,
    camera_to_working: Option<[[f32; 3]; 3]>,
) -> [f32; 3] {
    let scaled = [v[0] * scale, v[1] * scale, v[2] * scale];
    let out = match camera_to_working {
        Some(m) => apply_matrix_pixel(scaled, m),
        None => scaled,
    };
    out.map(|c| {
        if c.is_finite() {
            c.clamp(0.0, 1.0)
        } else {
            0.0
        }
    })
}

/// Step 8 of `docs/MATHS.md` (`docs/MATHS.md#step-8-export`, preview, the
/// negative before analysis): the negative display of `image` (linear
/// camera RGB after the caller's step 2, any pyramid level or part of one),
/// [`negative_display_pixel`] on every pixel. The result is linear, in the
/// working space when `camera_to_working` is given, for the caller's
/// display transform. Runs in parallel over row bands.
pub fn negative_display(
    image: &Image,
    scale: f32,
    camera_to_working: Option<[[f32; 3]; 3]>,
) -> Image {
    let mut out = image.clone();
    for_each_pixel_mut(&mut out, |px| {
        negative_display_pixel(px, scale, camera_to_working)
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use negg_film::analysis_grid;

    #[test]
    fn the_scale_maps_the_bright_percentile_to_white() {
        // A ramp of 1000 pixels, red from 0.0004 to 0.4, green and blue at
        // a half and a quarter of it. The three channels are pooled: the
        // 99.5th percentile of the 3000 samples is rank 2985, the 16th
        // largest: red at x = 984, 0.4 * 985 / 1000 = 0.394.
        let mut img = Image::new(1000, 1);
        for x in 0..1000 {
            let v = 0.4 * (x + 1) as f32 / 1000.0;
            img.set_pixel(x, 0, [v, v * 0.5, v * 0.25]);
        }
        let grid = analysis_grid(&img, 1024);
        let scale = negative_display_scale(&grid);
        assert!((1.0 / scale - 0.394).abs() < 1e-5, "{}", 1.0 / scale);
        assert_eq!(
            negative_display_scale(&analysis_grid(&Image::new(0, 0), 4)),
            1.0
        );
        let black = analysis_grid(&Image::new(4, 4), 4);
        assert_eq!(negative_display_scale(&black), 1.0);
    }

    #[test]
    fn the_display_scales_clips_and_does_not_invert() {
        let img = Image::from_vec(
            3,
            1,
            vec![0.1, 0.2, 0.4, 0.5, 0.6, 0.9, -0.1, f32::NAN, 0.0],
        )
        .unwrap();
        let out = negative_display(&img, 2.0, None);
        let want = [0.2, 0.4, 0.8, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0];
        for (got, want) in out.data().iter().zip(want) {
            assert!((got - want).abs() < 1e-6, "{:?}", out.data());
        }
        // A denser sample stays darker: no inversion.
        assert!(out.pixel(0, 0)[0] < out.pixel(1, 0)[0]);
        // The matrix applies after the scale.
        let swap = [[0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
        let p = negative_display_pixel([0.1, 0.2, 0.3], 2.0, Some(swap));
        assert!((p[0] - 0.4).abs() < 1e-6 && (p[1] - 0.2).abs() < 1e-6);
        assert_eq!(out.width(), 3);
    }
}
