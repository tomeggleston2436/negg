// SPDX-License-Identifier: MPL-2.0

//! The exposure anchor of step 5 of `docs/MATHS.md`, per ADR 0008
//! (R-anchor).
//!
//! The anchor is one scalar: the luma of normalised log exposure that
//! `negg-render` solves `x_0` to print at `P = 0.75`. It is taken from the
//! textured midtone cells of the analysis grid so that neither a blank
//! border nor a burnt-out sky decides the print exposure.

use negg_types::Image;

use crate::grid::AnalysisGrid;
use crate::stats::{median, percentile, trimmed_mean};

/// A cell with `cv` at least this is textured: the complement of the flat
/// threshold of step 3 of `docs/MATHS.md`.
const TEXTURED_CV: f32 = 0.03;

/// Lower and upper luma percentiles, over the textured cells, that bound
/// the midtones (R-anchor, ADR 0008).
const MIDTONE_LOW: f32 = 25.0;
const MIDTONE_HIGH: f32 = 75.0;

/// Fraction trimmed from each end of the midtone luma before the mean
/// (R-anchor, ADR 0008).
const TRIM: f32 = 0.10;

/// Fewer midtone cells than this and the anchor falls back to the median of
/// luma over every cell (R-anchor, ADR 0008).
const MIN_MIDTONE_CELLS: usize = 16;

/// The exposure anchor of step 5 of `docs/MATHS.md`
/// (`docs/MATHS.md#step-5-print-stage-the-tone-curve`), per ADR 0008
/// (R-anchor).
///
/// `grid` supplies the texture measure `cv` of each cell and `e_grid` is
/// the same grid after step 4 (normalised log exposure `e_c`, in `[-1, 0]`
/// for the no-profile gamma), computed by the caller with
/// `negg_render::invert` on `density(&grid.median)`. The two must have the
/// same dimensions.
///
/// Luma per cell is `l = (e_R + e_G + e_B) / 3`. Cells with `cv >= 0.03`
/// are textured. The midtones are the textured cells whose `l` lies between
/// the 25th and 75th percentiles (inclusive) of `l` over all textured cells.
/// The anchor is the 10% trimmed mean of `l` over the midtone cells. If
/// fewer than 16 midtone cells exist, it is the median of `l` over every
/// cell of the grid.
///
/// # Panics
///
/// Panics if `e_grid` does not match the grid's dimensions, or if the grid
/// has no cells.
pub fn exposure_anchor(grid: &AnalysisGrid, e_grid: &Image) -> f32 {
    assert!(grid.cells() > 0, "exposure anchor of an empty grid");
    assert!(
        e_grid.width() == grid.width() && e_grid.height() == grid.height(),
        "e_grid {}x{} does not match the {}x{} analysis grid",
        e_grid.width(),
        e_grid.height(),
        grid.width(),
        grid.height()
    );

    let luma: Vec<f32> = e_grid
        .data()
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| (p[0] + p[1] + p[2]) / 3.0)
        .collect();

    let mut textured: Vec<f32> = luma
        .iter()
        .zip(grid.cv.iter())
        .filter(|&(_, &cv)| cv >= TEXTURED_CV)
        .map(|(&l, _)| l)
        .collect();

    let mut midtones: Vec<f32> = if textured.is_empty() {
        Vec::new()
    } else {
        let low = percentile(&mut textured, MIDTONE_LOW);
        let high = percentile(&mut textured, MIDTONE_HIGH);
        textured
            .iter()
            .copied()
            .filter(|&l| l >= low && l <= high)
            .collect()
    };

    if midtones.len() < MIN_MIDTONE_CELLS {
        let mut all = luma;
        return median(&mut all);
    }
    trimmed_mean(&mut midtones, TRIM)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A grid whose `cv` is `cv` everywhere, with an `e_grid` filled by `f`.
    fn grid_and_e(
        w: usize,
        h: usize,
        cv: f32,
        f: impl Fn(usize, usize) -> [f32; 3],
    ) -> (AnalysisGrid, Image) {
        let mut e = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                e.set_pixel(x, y, f(x, y));
            }
        }
        let grid = AnalysisGrid {
            median: Image::new(w, h),
            cv: vec![cv; w * h],
            block: 1,
        };
        (grid, e)
    }

    #[test]
    fn uniform_textured_mid_grey_anchors_at_its_luma() {
        let (grid, e) = grid_and_e(8, 8, 0.05, |_, _| [-0.5; 3]);
        assert_eq!(exposure_anchor(&grid, &e), -0.5);
        // Luma is the mean of the three channels.
        let (grid, e) = grid_and_e(8, 8, 0.05, |_, _| [-0.3, -0.5, -0.7]);
        assert!((exposure_anchor(&grid, &e) + 0.5).abs() < 1e-6);
    }

    #[test]
    fn midtone_trimmed_mean_ignores_shadows_and_highlights() {
        // 100 textured cells with luma -1.0 .. 0.0 in steps of 0.01 plus a
        // few extreme outliers; midtones are the p25..p75 band around -0.5.
        let (mut grid, mut e) = grid_and_e(10, 10, 0.1, |x, y| {
            let l = -1.0 + (y * 10 + x) as f32 * 0.01;
            [l; 3]
        });
        e.set_pixel(0, 0, [-20.0; 3]);
        e.set_pixel(9, 9, [5.0; 3]);
        let a = exposure_anchor(&grid, &e);
        assert!((a + 0.5).abs() < 0.02, "{a}");

        // Flat cells are ignored even when their luma is extreme.
        for i in 0..20 {
            grid.cv[i] = 0.0;
            e.data_mut()[i * 3..i * 3 + 3].copy_from_slice(&[-30.0; 3]);
        }
        let a = exposure_anchor(&grid, &e);
        // Textured cells are now luma -0.80 .. 0.00 (and the +5 outlier):
        // the midtone band centres near -0.4.
        assert!((a + 0.4).abs() < 0.03, "{a}");
    }

    #[test]
    fn fewer_than_sixteen_midtones_falls_back_to_median_of_all() {
        // No textured cells at all: median of every cell.
        let (grid, e) = grid_and_e(5, 5, 0.0, |x, y| [-((x + y) as f32) * 0.1; 3]);
        let a = exposure_anchor(&grid, &e);
        // Luma over the 25 cells is -(x+y)/10; the median of x+y over a
        // 5x5 grid is 4 -> -0.4.
        assert!((a + 0.4).abs() < 1e-6, "{a}");

        // 20 textured cells give about 10 midtones: still under 16.
        let (mut grid, e) = grid_and_e(10, 10, 0.0, |x, y| [-((y * 10 + x) as f32) * 0.01; 3]);
        for cv in grid.cv.iter_mut().take(20) {
            *cv = 0.1;
        }
        let a = exposure_anchor(&grid, &e);
        // Median of luma 0.00 .. -0.99 over 100 cells is -0.495.
        assert!((a + 0.495).abs() < 1e-5, "{a}");
    }

    #[test]
    fn exactly_sixteen_midtones_use_the_trimmed_mean() {
        // 32 textured cells with luma 0, -0.01, ..., -0.31 (p25 = rank 8 ->
        // -0.24, p75 = rank 24 -> -0.08): 17 cells land inclusive in the
        // band, their 10% trimmed mean (drop 1 each end) is -0.16.
        let (mut grid, e) = grid_and_e(8, 8, 0.0, |x, y| [-((y * 8 + x) as f32) * 0.01; 3]);
        for cv in grid.cv.iter_mut().take(32) {
            *cv = 0.1;
        }
        let a = exposure_anchor(&grid, &e);
        assert!((a + 0.16).abs() < 1e-5, "{a}");
    }

    #[test]
    #[should_panic(expected = "does not match")]
    fn mismatched_dimensions_panic() {
        let (grid, _) = grid_and_e(4, 4, 0.1, |_, _| [0.0; 3]);
        let _ = exposure_anchor(&grid, &Image::new(4, 3));
    }
}
