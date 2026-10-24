// SPDX-License-Identifier: MPL-2.0

//! Film base (Dmin), Dmax and no-profile gamma estimators: steps 3 and 4
//! of `docs/MATHS.md`.
//!
//! The border rule follows ADR 0007 (R-border): the unexposed border of a
//! frame is trusted as the film base only when enough flat cells sit in the
//! outer band of the grid and those cells are no denser than the least
//! dense part of the whole frame. A flat wall or sky is flat but not the
//! least dense region, so it falls through to the percentile path.

use negg_types::DminSource;

use crate::grid::{AnalysisGrid, density};
use crate::stats::{median, percentile};

/// Width of the border band as a fraction of each grid dimension, on every
/// edge. Step 3 of `docs/MATHS.md`: "outer 15%".
const BORDER_BAND: f64 = 0.15;

/// A cell with `cv` below this is flat. Step 3 of `docs/MATHS.md`.
const FLAT_CV: f32 = 0.03;

/// Minimum number of flat cells in the border band before the border path
/// is trusted (R-border, ADR 0007).
const MIN_FLAT_CELLS: u32 = 20;

/// Minimum fraction of the border band's cells that must be flat before
/// the border path is trusted (R-border, ADR 0007).
const MIN_FLAT_FRACTION: f64 = 0.05;

/// The flat border cells' median density may exceed the grid's 0.5th
/// percentile by at most this much, per channel, before the border is
/// judged not to be the least dense region (R-border, ADR 0007).
const BORDER_ABOVE_FLOOR: f32 = 0.05;

/// Percentile of grid density that is the fallback Dmin. Step 3.
const DMIN_PERCENTILE: f32 = 0.5;

/// Percentile of grid density that is Dmax. Step 3.
const DMAX_PERCENTILE: f32 = 99.5;

/// Floor for the no-profile gamma, in log-D. See [`no_profile_gamma`].
const GAMMA_FLOOR: f32 = 0.05;

/// The result of [`estimate_dmin`]: the film base per channel and which
/// path of step 3 of `docs/MATHS.md` produced it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DminEstimate {
    /// Film base density per channel.
    pub value: [f32; 3],
    /// [`DminSource::Border`] or [`DminSource::Percentile`].
    pub source: DminSource,
    /// Number of flat cells (`cv < 0.03`) found in the outer 15% band of the
    /// grid, whichever path was taken. Zero on an empty grid.
    pub flat_border_cells: u32,
}

/// Per-channel columns of a grid image's density, as three `Vec<f32>`.
fn channels(cells: impl Iterator<Item = [f32; 3]>) -> [Vec<f32>; 3] {
    let mut out = [Vec::new(), Vec::new(), Vec::new()];
    for [r, g, b] in cells {
        out[0].push(r);
        out[1].push(g);
        out[2].push(b);
    }
    out
}

/// Whether grid cell `(x, y)` lies in the outer 15% band of a `w x h` grid.
/// The band is `ceil(0.15 * w)` cells wide on the left and right and
/// `ceil(0.15 * h)` cells high on the top and bottom, so a small grid still
/// has at least one cell of band on every edge.
fn in_border_band(x: usize, y: usize, w: usize, h: usize) -> bool {
    let bw = (BORDER_BAND * w as f64).ceil() as usize;
    let bh = (BORDER_BAND * h as f64).ceil() as usize;
    x < bw || x + bw >= w || y < bh || y + bh >= h
}

/// Estimates Dmin, the film base density per channel, from the analysis
/// grid: step 3 of `docs/MATHS.md`
/// (`docs/MATHS.md#step-3-density-and-film-base`), with the border rule of
/// ADR 0007 (R-border).
///
/// The border path is taken when both hold:
///
/// 1. at least 20 cells, and at least 5% of the cells, in the outer 15%
///    band of the grid are flat (`cv < 0.03`);
/// 2. the per-channel median density of those flat cells is at most 0.05
///    log-D above the 0.5th percentile of density over the whole grid, in
///    every channel.
///
/// Then `Dmin_c` is that median and the source is [`DminSource::Border`].
/// Otherwise `Dmin_c` is the 0.5th percentile of density over the grid and
/// the source is [`DminSource::Percentile`]. Rule 2 is what stops a flat
/// wall or sky, which is flat but denser than the unexposed base, from
/// posing as a border.
///
/// # Panics
///
/// Panics if the grid has no cells.
pub fn estimate_dmin(grid: &AnalysisGrid) -> DminEstimate {
    assert!(grid.cells() > 0, "Dmin of an empty grid");
    let dens = density(&grid.median);
    let (w, h) = (grid.width(), grid.height());

    let mut all = channels(dens.data().as_chunks::<3>().0.iter().copied());
    let floor = [
        percentile(&mut all[0], DMIN_PERCENTILE),
        percentile(&mut all[1], DMIN_PERCENTILE),
        percentile(&mut all[2], DMIN_PERCENTILE),
    ];

    let mut band_cells = 0usize;
    let mut flat = channels(
        dens.data()
            .as_chunks::<3>()
            .0
            .iter()
            .zip(grid.cv.iter())
            .enumerate()
            .filter(|&(i, (_, &cv))| {
                let in_band = in_border_band(i % w, i / w, w, h);
                band_cells += usize::from(in_band);
                in_band && cv < FLAT_CV
            })
            .map(|(_, (&p, _))| p),
    );
    let flat_count = flat[0].len();
    let flat_border_cells = u32::try_from(flat_count).unwrap_or(u32::MAX);

    let enough = flat_border_cells >= MIN_FLAT_CELLS
        && flat_count as f64 >= MIN_FLAT_FRACTION * band_cells as f64;
    if enough {
        let border = [
            median(&mut flat[0]),
            median(&mut flat[1]),
            median(&mut flat[2]),
        ];
        let least_dense = (0..3).all(|c| border[c] <= floor[c] + BORDER_ABOVE_FLOOR);
        if least_dense {
            return DminEstimate {
                value: border,
                source: DminSource::Border,
                flat_border_cells,
            };
        }
    }
    DminEstimate {
        value: floor,
        source: DminSource::Percentile,
        flat_border_cells,
    }
}

/// Estimates Dmax per channel as the 99.5th percentile of density over the
/// analysis grid: step 3 of `docs/MATHS.md`
/// (`docs/MATHS.md#step-3-density-and-film-base`).
///
/// # Panics
///
/// Panics if the grid has no cells.
pub fn estimate_dmax(grid: &AnalysisGrid) -> [f32; 3] {
    assert!(grid.cells() > 0, "Dmax of an empty grid");
    let dens = density(&grid.median);
    let mut all = channels(dens.data().as_chunks::<3>().0.iter().copied());
    [
        percentile(&mut all[0], DMAX_PERCENTILE),
        percentile(&mut all[1], DMAX_PERCENTILE),
        percentile(&mut all[2], DMAX_PERCENTILE),
    ]
}

/// The no-profile layer gamma of step 4 of `docs/MATHS.md`
/// (`docs/MATHS.md#step-4-inversion-to-relative-log-exposure`):
/// `gamma_c = Dmax_c - Dmin_c`, so that each channel spans `[0, -1]` after
/// inversion.
///
/// Each channel is floored at 0.05 log-D. Step 4 divides by `gamma_c`, so
/// a zero range (a blank or uniform frame, or a channel that clipped) would
/// divide by zero; 0.05 is a sanity floor well below any real film's
/// density range and is not a tuned value. TODO(measure): confirm against
/// the thinnest negatives in the eval set once it exists.
pub fn no_profile_gamma(dmin: [f32; 3], dmax: [f32; 3]) -> [f32; 3] {
    [
        (dmax[0] - dmin[0]).max(GAMMA_FLOOR),
        (dmax[1] - dmin[1]).max(GAMMA_FLOOR),
        (dmax[2] - dmin[2]).max(GAMMA_FLOOR),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use negg_types::Image;

    /// A grid with every cell at linear `value` and the given `cv`.
    fn uniform_grid(w: usize, h: usize, value: [f32; 3], cv: f32) -> AnalysisGrid {
        let mut median = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                median.set_pixel(x, y, value);
            }
        }
        AnalysisGrid {
            median,
            cv: vec![cv; w * h],
            block: 1,
            source_width: w,
            source_height: h,
        }
    }

    #[test]
    fn border_band_is_the_outer_fifteen_percent_with_ceiling() {
        // 20 wide: band = ceil(3) = 3 columns each side.
        assert!(in_border_band(0, 10, 20, 20));
        assert!(in_border_band(2, 10, 20, 20));
        assert!(!in_border_band(3, 10, 20, 20));
        assert!(!in_border_band(16, 10, 20, 20));
        assert!(in_border_band(17, 10, 20, 20));
        assert!(in_border_band(19, 10, 20, 20));
        // Rows too.
        assert!(in_border_band(10, 0, 20, 20));
        assert!(in_border_band(10, 19, 20, 20));
        assert!(!in_border_band(10, 10, 20, 20));
        // A tiny grid still has a band on every edge.
        assert!(in_border_band(0, 1, 3, 3));
        assert!(!in_border_band(1, 1, 3, 3));
    }

    #[test]
    fn uniform_flat_grid_takes_the_border_path() {
        // Linear 0.5 -> density 0.30103 everywhere, all flat.
        let grid = uniform_grid(40, 30, [0.5; 3], 0.0);
        let est = estimate_dmin(&grid);
        assert_eq!(est.source, DminSource::Border);
        assert!(est.flat_border_cells >= 20);
        for c in 0..3 {
            assert!(
                (est.value[c] - std::f32::consts::LOG10_2).abs() < 1e-5,
                "{:?}",
                est.value
            );
        }
        assert_eq!(estimate_dmax(&grid)[1], est.value[1]);
    }

    #[test]
    fn textured_everywhere_takes_the_percentile_path() {
        let grid = uniform_grid(40, 30, [0.25; 3], 0.1);
        let est = estimate_dmin(&grid);
        assert_eq!(est.source, DminSource::Percentile);
        assert_eq!(est.flat_border_cells, 0);
        assert!((est.value[0] - 0.60206).abs() < 1e-5);
    }

    #[test]
    fn too_few_flat_cells_falls_back_to_percentile() {
        // 10 flat border cells on a 40x30 grid: fewer than 20.
        let mut grid = uniform_grid(40, 30, [0.5; 3], 0.1);
        for x in 0..10 {
            grid.cv[x] = 0.0;
        }
        let est = estimate_dmin(&grid);
        assert_eq!(est.flat_border_cells, 10);
        assert_eq!(est.source, DminSource::Percentile);

        // 25 flat cells on a 200x200 grid: at least 20 but under 5% of the
        // band (band = 200*200 - 140*140 = 20400 cells, 5% = 1020).
        let mut grid = uniform_grid(200, 200, [0.5; 3], 0.1);
        for x in 0..25 {
            grid.cv[x] = 0.0;
        }
        let est = estimate_dmin(&grid);
        assert_eq!(est.flat_border_cells, 25);
        assert_eq!(est.source, DminSource::Percentile);
    }

    #[test]
    fn flat_cells_denser_than_the_floor_are_not_a_border() {
        // Interior is textured at linear 0.8 (density 0.097); the whole top
        // band is flat at linear 0.4 (density 0.398): a sky, not a border.
        let mut grid = uniform_grid(40, 30, [0.8; 3], 0.1);
        for y in 0..5 {
            for x in 0..40 {
                grid.median.set_pixel(x, y, [0.4; 3]);
                grid.cv[y * 40 + x] = 0.0;
            }
        }
        let est = estimate_dmin(&grid);
        assert_eq!(est.flat_border_cells, 200);
        assert_eq!(est.source, DminSource::Percentile);
        assert!((est.value[0] - 0.09691).abs() < 1e-4, "{:?}", est.value);
    }

    #[test]
    fn flat_cells_within_tolerance_of_the_floor_are_a_border() {
        // Interior textured at linear 0.5; top band flat at linear 0.45,
        // which is 0.046 log-D denser: within the 0.05 tolerance.
        let mut grid = uniform_grid(40, 30, [0.5; 3], 0.1);
        for y in 0..5 {
            for x in 0..40 {
                grid.median.set_pixel(x, y, [0.45; 3]);
                grid.cv[y * 40 + x] = 0.0;
            }
        }
        let est = estimate_dmin(&grid);
        assert_eq!(est.source, DminSource::Border);
        assert!((est.value[0] - 0.34679).abs() < 1e-4, "{:?}", est.value);
    }

    #[test]
    fn interior_flat_cells_do_not_count() {
        // Flat cells only in the centre: the border band has none.
        let mut grid = uniform_grid(40, 30, [0.5; 3], 0.1);
        for y in 10..20 {
            for x in 10..30 {
                grid.cv[y * 40 + x] = 0.0;
            }
        }
        let est = estimate_dmin(&grid);
        assert_eq!(est.flat_border_cells, 0);
        assert_eq!(est.source, DminSource::Percentile);
    }

    #[test]
    fn dmin_and_dmax_are_per_channel_percentiles() {
        // 200 cells with linear values ramping so densities are distinct.
        let w = 200;
        let mut grid = uniform_grid(w, 1, [1.0; 3], 0.1);
        for x in 0..w {
            let d = x as f32 * 0.01;
            grid.median.set_pixel(
                x,
                0,
                [10f32.powf(-d), 10f32.powf(-d - 0.1), 10f32.powf(-d - 0.2)],
            );
        }
        let dmin = estimate_dmin(&grid);
        assert_eq!(dmin.source, DminSource::Percentile);
        // rank ceil(0.005 * 200) = 1 -> the lowest density per channel.
        assert!((dmin.value[0] - 0.0).abs() < 1e-5, "{:?}", dmin.value);
        assert!((dmin.value[1] - 0.1).abs() < 1e-5, "{:?}", dmin.value);
        assert!((dmin.value[2] - 0.2).abs() < 1e-5, "{:?}", dmin.value);
        // rank ceil(0.995 * 200) = 199 -> density 1.98 (index 198).
        let dmax = estimate_dmax(&grid);
        assert!((dmax[0] - 1.98).abs() < 1e-4, "{dmax:?}");
        assert!((dmax[1] - 2.08).abs() < 1e-4, "{dmax:?}");
        assert!((dmax[2] - 2.18).abs() < 1e-4, "{dmax:?}");
    }

    #[test]
    fn no_profile_gamma_is_range_with_floor() {
        let g = no_profile_gamma([0.2, 0.5, 0.7], [1.9, 2.2, 2.3]);
        for (got, want) in g.iter().zip([1.7, 1.7, 1.6]) {
            assert!((got - want).abs() < 1e-6, "{g:?}");
        }
        assert_eq!(no_profile_gamma([0.5; 3], [0.5; 3]), [0.05; 3]);
        assert_eq!(no_profile_gamma([0.6; 3], [0.5; 3]), [0.05; 3]);
        let g = no_profile_gamma([0.5, 0.5, 0.5], [0.52, 0.56, 0.5]);
        assert_eq!(g[0], 0.05, "{g:?}");
        assert!((g[1] - 0.06).abs() < 1e-6, "{g:?}");
        assert_eq!(g[2], 0.05, "{g:?}");
    }

    #[test]
    #[should_panic(expected = "empty")]
    fn dmin_of_empty_grid_panics() {
        let grid = AnalysisGrid {
            median: Image::new(0, 0),
            cv: Vec::new(),
            block: 1,
            source_width: 0,
            source_height: 0,
        };
        let _ = estimate_dmin(&grid);
    }
}
