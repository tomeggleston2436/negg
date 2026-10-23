// SPDX-License-Identifier: MPL-2.0

//! Step 6 of `docs/MATHS.md`: auto colour (cast removal), per ADR 0009.
//!
//! The fit runs on the analysis grid after step 4, where every cell is a
//! normalised log exposure triple `e = (e_R, e_G, e_B)`, nominally in
//! `[-1, 0]`. After ADR 0011 the print is brighter where `e` is more
//! negative, so luma and hue are read on the print-space orientation
//! `p = -e` (ADR 0009 as amended by Ruling 30): a cell that prints green
//! has `p_G` largest. For each cell:
//!
//! ```text
//! p      = -e
//! luma   l = (p_R + p_G + p_B) / 3
//! chroma c = max(p) - min(p)          (the same on e and on p)
//! hue      = HSV hue angle of (p_R, p_G, p_B): red 0, green 120, blue 240,
//!            undefined when c < 1e-4
//! ```
//!
//! Cells are split into shadows `[p0, p20)`, mids `[p20, p80)` and
//! highlights `[p80, p100]` of `l`, so the shadows are the cells that
//! print darkest. In each band the cells whose hue lies
//! in the foliage sector (90 to 160 degrees) or the sky sector (200 to 260
//! degrees) are dropped, and the lowest-chroma 30% of the rest are kept. A
//! cell with undefined hue (chroma below 1e-4) cannot fall in either sector
//! and is the most neutral cell available, so it is kept. The kept cells of the three bands are pooled and fit
//! by ordinary least squares:
//!
//! ```text
//! e_R = s_R * e_G + o_R
//! e_B = s_B * e_G + o_B
//! ```
//!
//! with the slopes clamped to `[0.8, 1.2]`. Applying the correction inverts
//! the fitted lines so that the selected cells land on `e_G`:
//!
//! ```text
//! e_R <- (e_R - o_R) / s_R
//! e_B <- (e_B - o_B) / s_B
//! ```
//!
//! Fewer than 8 surviving cells gives the identity and `cells_used = 0`.

use negg_types::{CastCorrection, Image};

use crate::par::for_each_pixel_mut;

/// Below this chroma the hue is undefined; such a cell is neutral and is
/// never excluded by the hue sectors.
const CHROMA_UNDEFINED: f32 = 1e-4;
/// Foliage hue sector, degrees, inclusive.
const FOLIAGE_HUE: (f32, f32) = (90.0, 160.0);
/// Sky hue sector, degrees, inclusive.
const SKY_HUE: (f32, f32) = (200.0, 260.0);
/// Luma percentile between shadows and mids.
const SHADOW_PERCENTILE: f32 = 20.0;
/// Luma percentile between mids and highlights.
const HIGHLIGHT_PERCENTILE: f32 = 80.0;
/// Fraction of each band's eligible cells kept, lowest chroma first.
const LOW_CHROMA_FRACTION: f64 = 0.3;
/// Minimum pooled cells for a fit; fewer gives the identity.
const MIN_CELLS: usize = 8;
/// Clamp range of the fitted slopes.
const SLOPE_RANGE: (f64, f64) = (0.8, 1.2);

/// One grid cell's statistics for the fit.
#[derive(Clone, Copy, Debug)]
struct Cell {
    /// The cell's normalised log exposure, which the lines are fitted on.
    e: [f32; 3],
    /// Mean of `-e`: higher prints brighter.
    luma: f32,
    chroma: f32,
    /// HSV hue of `-e` in degrees `[0, 360)`, `None` when chroma is below
    /// [`CHROMA_UNDEFINED`].
    hue: Option<f32>,
}

impl Cell {
    /// Luma, chroma and hue of the print-space triple `-e` (ADR 0009 as
    /// amended), keeping `e` itself for the fit.
    fn from_e(e: [f32; 3]) -> Cell {
        let [r, g, b] = [-e[0], -e[1], -e[2]];
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let chroma = max - min;
        let hue = if chroma < CHROMA_UNDEFINED {
            None
        } else {
            let h = if max == r {
                60.0 * ((g - b) / chroma)
            } else if max == g {
                60.0 * ((b - r) / chroma + 2.0)
            } else {
                60.0 * ((r - g) / chroma + 4.0)
            };
            Some(if h < 0.0 { h + 360.0 } else { h })
        };
        Cell {
            e,
            luma: (r + g + b) / 3.0,
            chroma,
            hue,
        }
    }

    /// Whether the cell may take part in the fit: its hue is undefined
    /// (neutral) or lies outside the foliage and sky sectors.
    fn eligible(&self) -> bool {
        match self.hue {
            None => true,
            Some(h) => {
                !((FOLIAGE_HUE.0..=FOLIAGE_HUE.1).contains(&h)
                    || (SKY_HUE.0..=SKY_HUE.1).contains(&h))
            }
        }
    }
}

/// Nearest-rank percentile of `sorted` (ascending), `p` in `[0, 100]`.
/// `sorted` must not be empty.
fn percentile_sorted(sorted: &[f32], p: f32) -> f32 {
    let n = sorted.len();
    let rank = ((f64::from(p) / 100.0) * n as f64).ceil() as usize;
    sorted[rank.clamp(1, n) - 1]
}

/// Sorts `band` by chroma and keeps the lowest [`LOW_CHROMA_FRACTION`]
/// (rounded up, so a non-empty band keeps at least one cell).
fn keep_lowest_chroma(mut band: Vec<Cell>) -> Vec<Cell> {
    band.sort_by(|a, b| a.chroma.total_cmp(&b.chroma));
    let keep = (band.len() as f64 * LOW_CHROMA_FRACTION).ceil() as usize;
    band.truncate(keep);
    band
}

/// Ordinary least squares of `y = s * x + o` over `(x, y)` pairs, with the
/// slope clamped to [`SLOPE_RANGE`] and the offset then taken through the
/// centroid so the pooled cells still land on `e_G` on average.
fn fit_line(xs: &[f64], ys: &[f64]) -> (f32, f32) {
    let n = xs.len() as f64;
    let mean_x = xs.iter().sum::<f64>() / n;
    let mean_y = ys.iter().sum::<f64>() / n;
    let mut sxx = 0.0;
    let mut sxy = 0.0;
    for (x, y) in xs.iter().zip(ys) {
        let dx = x - mean_x;
        sxx += dx * dx;
        sxy += dx * (y - mean_y);
    }
    // A degenerate spread in e_G leaves the slope unidentified; keep 1.
    let slope = if sxx > 0.0 { sxy / sxx } else { 1.0 };
    let slope = slope.clamp(SLOPE_RANGE.0, SLOPE_RANGE.1);
    let offset = mean_y - slope * mean_x;
    (slope as f32, offset as f32)
}

/// Step 6 of `docs/MATHS.md` (ADR 0009, hue and luma on `-e` as amended):
/// fits the cast correction on the analysis grid `e_grid` (the grid after
/// step 4, normalised log exposure).
///
/// See the module documentation for the selection and the fit. Returns the
/// identity with `cells_used = 0` when fewer than 8 cells survive
/// selection; otherwise `cells_used` is the number of pooled cells. Cells
/// with non-finite values are ignored.
#[must_use]
pub fn fit_cast(e_grid: &Image) -> CastCorrection {
    let cells: Vec<Cell> = e_grid
        .data()
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| Cell::from_e(*p))
        .filter(|c| c.e.iter().all(|v| v.is_finite()))
        .collect();
    if cells.is_empty() {
        return CastCorrection::IDENTITY;
    }

    let mut lumas: Vec<f32> = cells.iter().map(|c| c.luma).collect();
    lumas.sort_by(f32::total_cmp);
    let p20 = percentile_sorted(&lumas, SHADOW_PERCENTILE);
    let p80 = percentile_sorted(&lumas, HIGHLIGHT_PERCENTILE);

    let mut bands: [Vec<Cell>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for cell in cells.iter().filter(|c| c.eligible()) {
        let band = if cell.luma < p20 {
            0
        } else if cell.luma < p80 {
            1
        } else {
            2
        };
        bands[band].push(*cell);
    }

    let pooled: Vec<Cell> = bands.into_iter().flat_map(keep_lowest_chroma).collect();
    if pooled.len() < MIN_CELLS {
        return CastCorrection::IDENTITY;
    }

    let g: Vec<f64> = pooled.iter().map(|c| f64::from(c.e[1])).collect();
    let r: Vec<f64> = pooled.iter().map(|c| f64::from(c.e[0])).collect();
    let b: Vec<f64> = pooled.iter().map(|c| f64::from(c.e[2])).collect();
    let (slope_r, offset_r) = fit_line(&g, &r);
    let (slope_b, offset_b) = fit_line(&g, &b);
    CastCorrection {
        slope_r,
        offset_r,
        slope_b,
        offset_b,
        cells_used: pooled.len() as u32,
    }
}

/// Step 6 of `docs/MATHS.md` for one pixel: `e_R <- (e_R - o_R) / s_R`,
/// `e_B <- (e_B - o_B) / s_B`, `e_G` unchanged.
///
/// A `CastCorrection` can arrive from a sidecar with any public field
/// values, so a slope that is zero or not finite is treated as the identity
/// for that channel instead of dividing by it.
///
/// This is the kernel shared by [`apply_cast`] and [`crate::render_full`].
#[inline]
#[must_use]
pub fn apply_cast_pixel(e: [f32; 3], cast: &CastCorrection) -> [f32; 3] {
    [
        correct_channel(e[0], cast.slope_r, cast.offset_r),
        e[1],
        correct_channel(e[2], cast.slope_b, cast.offset_b),
    ]
}

/// `(e - offset) / slope`, or `e` unchanged when `slope` is zero or not
/// finite.
#[inline]
fn correct_channel(e: f32, slope: f32, offset: f32) -> f32 {
    if slope != 0.0 && slope.is_finite() {
        (e - offset) / slope
    } else {
        e
    }
}

/// Step 6 of `docs/MATHS.md`: applies `cast` to every pixel of the
/// full-resolution `e` image in place, before step 5. Runs in parallel over
/// row bands.
pub fn apply_cast(e: &mut Image, cast: &CastCorrection) {
    for_each_pixel_mut(e, |px| apply_cast_pixel(px, cast));
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// A deterministic spread of `e_G` values over `[-1, 0]` for a grid of
    /// `n` cells, in a scrambled order so the bands mix positions.
    fn green_ramp(n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let j = (i * 7919) % n;
                -(j as f32) / (n as f32 - 1.0)
            })
            .collect()
    }

    /// Builds a `w` by `h` grid where `e_R = s_r * e_G + o_r` and
    /// `e_B = s_b * e_G + o_b`, with `outliers` extra strongly red cells.
    fn cast_grid(w: usize, h: usize, s_r: f32, o_r: f32, s_b: f32, o_b: f32) -> Image {
        let n = w * h;
        let g = green_ramp(n);
        let mut img = Image::new(w, h);
        for (i, px) in img.data_mut().as_chunks_mut::<3>().0.iter_mut().enumerate() {
            let eg = g[i];
            px[0] = s_r * eg + o_r;
            px[1] = eg;
            px[2] = s_b * eg + o_b;
        }
        img
    }

    /// The cell for a print-space triple `p`, that is `e = -p`.
    fn print_cell(p: [f32; 3]) -> Cell {
        Cell::from_e([-p[0], -p[1], -p[2]])
    }

    #[test]
    fn hue_follows_the_hsv_sectors_on_minus_e() {
        let red = print_cell([0.5, 0.2, 0.2]);
        let green = print_cell([0.2, 0.5, 0.2]);
        let blue = print_cell([0.2, 0.2, 0.5]);
        let yellow = print_cell([0.5, 0.5, 0.2]);
        let magenta = print_cell([0.5, 0.2, 0.5]);
        assert_eq!(red.hue, Some(0.0));
        assert_eq!(green.hue, Some(120.0));
        assert_eq!(blue.hue, Some(240.0));
        assert_eq!(yellow.hue, Some(60.0));
        assert_eq!(magenta.hue, Some(300.0));
        assert!(red.eligible());
        assert!(!green.eligible());
        assert!(!blue.eligible());
        assert!(yellow.eligible());
        assert!(magenta.eligible());
        // A cell whose e_G is the largest (least negative) is magenta in
        // print, not green, and is not excluded as foliage.
        let e_green = Cell::from_e([-0.5, -0.2, -0.5]);
        assert_eq!(e_green.hue, Some(300.0));
        assert!(e_green.eligible());
        let grey = Cell::from_e([-0.3, -0.3, -0.3 + 5e-5]);
        assert_eq!(grey.hue, None);
        assert!(grey.eligible());
        // Luma is read on -e: higher prints brighter.
        assert!((grey.luma - 0.3).abs() < 1e-4);
        assert!(print_cell([0.8; 3]).luma > print_cell([0.1; 3]).luma);
        assert!((red.chroma - 0.3).abs() < 1e-6);
    }

    #[test]
    fn nearest_rank_percentile() {
        let v = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(percentile_sorted(&v, 0.0), 1.0);
        assert_eq!(percentile_sorted(&v, 20.0), 1.0);
        assert_eq!(percentile_sorted(&v, 50.0), 3.0);
        assert_eq!(percentile_sorted(&v, 80.0), 4.0);
        assert_eq!(percentile_sorted(&v, 100.0), 5.0);
    }

    #[test]
    fn recovers_a_known_affine_cast() {
        // e_R = 1.1 e_G - 0.03 and e_B = 0.9 e_G + 0.05: warm in print
        // (p_R largest, p_B smallest), outside both hue sectors.
        let mut grid = cast_grid(32, 24, 1.1, -0.03, 0.9, 0.05);
        // A handful of strongly red cells (dust, a red object) with high
        // chroma: the 30% rule must drop them.
        for i in 0..40 {
            let x = (i * 13) % 32;
            let y = (i * 7) % 24;
            let eg = grid.pixel(x, y)[1];
            grid.set_pixel(x, y, [eg - 0.5, eg, eg]);
        }
        let cast = fit_cast(&grid);
        assert!(
            (cast.slope_r - 1.1).abs() < 1e-3,
            "slope_r {}",
            cast.slope_r
        );
        assert!(
            (cast.offset_r + 0.03).abs() < 1e-3,
            "offset_r {}",
            cast.offset_r
        );
        assert!(
            (cast.slope_b - 0.9).abs() < 1e-3,
            "slope_b {}",
            cast.slope_b
        );
        assert!(
            (cast.offset_b - 0.05).abs() < 1e-3,
            "offset_b {}",
            cast.offset_b
        );
        assert!(cast.cells_used >= 8);
        assert!(cast.cells_used < 32 * 24);

        let mut fixed = cast_grid(32, 24, 1.1, -0.03, 0.9, 0.05);
        apply_cast(&mut fixed, &cast);
        for px in fixed.data().as_chunks::<3>().0 {
            assert!((px[0] - px[1]).abs() < 1e-4, "{px:?}");
            assert!((px[2] - px[1]).abs() < 1e-4, "{px:?}");
        }
    }

    #[test]
    fn all_foliage_gives_identity() {
        // Green in print: e_G is the most negative channel.
        let mut grid = Image::new(16, 16);
        for px in grid.data_mut().as_chunks_mut::<3>().0.iter_mut() {
            px[0] = -0.3;
            px[1] = -0.6;
            px[2] = -0.3;
        }
        let cast = fit_cast(&grid);
        assert_eq!(cast, CastCorrection::IDENTITY);
        assert_eq!(cast.cells_used, 0);
    }

    #[test]
    fn all_sky_gives_identity() {
        let mut grid = Image::new(16, 16);
        for (i, px) in grid
            .data_mut()
            .as_chunks_mut::<3>()
            .0
            .iter_mut()
            .enumerate()
        {
            // Blue in print (hue 200 on -e): e_B is the most negative.
            let g = -(i as f32) / 256.0;
            px[0] = g + 0.2;
            px[1] = g;
            px[2] = g - 0.1;
        }
        assert_eq!(fit_cast(&grid), CastCorrection::IDENTITY);
    }

    #[test]
    fn neutral_grey_among_foliage_fits_near_identity() {
        // The foliage scene of the phase 1 review: mostly foliage that is
        // green in print (e_G the most negative channel, hue about 144 on
        // -e), with one cell in seven a neutral grey. Read on e, the
        // foliage would be magenta, pass the sector test, and its
        // lowest-chroma cells would drag the fit; read on -e it is
        // excluded and only the greys are fitted.
        let (w, h) = (32, 24);
        let n = w * h;
        let g = green_ramp(n);
        let mut grid = Image::new(w, h);
        for (i, px) in grid
            .data_mut()
            .as_chunks_mut::<3>()
            .0
            .iter_mut()
            .enumerate()
        {
            let eg = g[i];
            if i % 7 == 0 {
                px.copy_from_slice(&[eg; 3]);
            } else {
                let a = 0.05 + 0.15 * ((i * 31) % 97) as f32 / 96.0;
                px.copy_from_slice(&[eg + a, eg, eg + 0.6 * a]);
            }
        }
        let foliage = Cell::from_e(grid.pixel(1, 0));
        assert!(!foliage.eligible(), "{foliage:?}");
        let cast = fit_cast(&grid);
        assert!(cast.cells_used >= 8, "{cast:?}");
        assert!((cast.slope_r - 1.0).abs() < 1e-3, "{cast:?}");
        assert!((cast.slope_b - 1.0).abs() < 1e-3, "{cast:?}");
        assert!(cast.offset_r.abs() < 1e-3, "{cast:?}");
        assert!(cast.offset_b.abs() < 1e-3, "{cast:?}");
    }

    #[test]
    fn neutral_grid_keeps_its_cells_and_fits_the_identity() {
        let mut grid = Image::new(16, 16);
        for (i, px) in grid
            .data_mut()
            .as_chunks_mut::<3>()
            .0
            .iter_mut()
            .enumerate()
        {
            let g = -(i as f32) / 256.0;
            px.copy_from_slice(&[g; 3]);
        }
        let cast = fit_cast(&grid);
        assert!(cast.cells_used > 0, "{cast:?}");
        assert!((cast.slope_r - 1.0).abs() < 1e-6, "{cast:?}");
        assert!((cast.slope_b - 1.0).abs() < 1e-6, "{cast:?}");
        assert!(cast.offset_r.abs() < 1e-6, "{cast:?}");
        assert!(cast.offset_b.abs() < 1e-6, "{cast:?}");
        // A flat neutral grid has no spread in e_G: the slope is left at 1.
        // Every cell shares one luma, so all 64 land in one band and the
        // 30% rule keeps ceil(19.2) = 20 of them.
        let mut flat = Image::new(8, 8);
        flat.data_mut().fill(-0.4);
        let cast = fit_cast(&flat);
        assert_eq!(cast.cells_used, 20);
        assert_eq!(cast.slope_r, 1.0);
        assert_eq!(cast.offset_r, 0.0);
        assert_eq!(fit_cast(&Image::new(0, 0)), CastCorrection::IDENTITY);
    }

    #[test]
    fn fewer_than_eight_cells_gives_identity() {
        let grid = cast_grid(3, 2, 0.9, 0.05, 1.0, 0.0);
        assert_eq!(fit_cast(&grid), CastCorrection::IDENTITY);
        // 30% of 24 eligible cells per band: 24 cells in 3 bands keep
        // about 8, which is enough; 20 cells keep fewer and are not.
        let grid = cast_grid(5, 4, 0.9, 0.05, 1.0, 0.0);
        assert_eq!(fit_cast(&grid).cells_used, 0);
    }

    #[test]
    fn slopes_are_clamped() {
        // Warm in print, outside both hue sectors, with slopes past the
        // clamp on both sides.
        let cast = fit_cast(&cast_grid(32, 24, 1.5, 0.0, 0.5, 0.1));
        assert!(
            (cast.slope_r - 1.2).abs() < 1e-6,
            "slope_r {}",
            cast.slope_r
        );
        assert!(
            (cast.slope_b - 0.8).abs() < 1e-6,
            "slope_b {}",
            cast.slope_b
        );
        assert!(cast.cells_used >= 8);
        // After the clamp the offset goes through the centroid of the pooled
        // cells (ADR 0009 as amended): offset = mean_y - clamped_slope * mean_g.
        // The pooled cells lie exactly on e_R = 1.5 e_G and
        // e_B = 0.5 e_G + 0.1, so offset_r = 0.3 mean_g and
        // offset_b = 0.1 - 0.3 mean_g, which ties the two offsets together.
        assert!(
            (cast.offset_b - (0.1 - cast.offset_r)).abs() < 1e-5,
            "{cast:?}"
        );
        // And the values themselves, for this grid's selection.
        const OFFSET_R: f32 = -0.1042;
        assert!(
            (cast.offset_r - OFFSET_R).abs() < 1e-4,
            "offset_r {}",
            cast.offset_r
        );
        assert!(
            (cast.offset_b - (0.1 - OFFSET_R)).abs() < 1e-4,
            "offset_b {}",
            cast.offset_b
        );
    }

    #[test]
    fn zero_or_non_finite_slope_is_identity_for_that_channel() {
        let cast = CastCorrection {
            slope_r: 0.0,
            offset_r: 0.3,
            slope_b: f32::NAN,
            offset_b: 0.3,
            cells_used: 8,
        };
        assert_eq!(
            apply_cast_pixel([-0.5, -0.4, -0.3], &cast),
            [-0.5, -0.4, -0.3]
        );
    }

    #[test]
    fn apply_cast_matches_per_pixel_and_identity_is_a_no_op() {
        let cast = CastCorrection {
            slope_r: 0.95,
            offset_r: 0.02,
            slope_b: 1.05,
            offset_b: -0.01,
            cells_used: 10,
        };
        let mut img = cast_grid(9, 7, 1.0, 0.0, 1.0, 0.0);
        let before = img.clone();
        apply_cast(&mut img, &cast);
        for (a, b) in img
            .data()
            .as_chunks::<3>()
            .0
            .iter()
            .zip(before.data().as_chunks::<3>().0)
        {
            assert_eq!(*a, apply_cast_pixel(*b, &cast));
        }
        let mut same = before.clone();
        apply_cast(&mut same, &CastCorrection::IDENTITY);
        assert_eq!(same, before);
    }

    proptest! {
        /// Applying the correction undoes the fitted line exactly.
        #[test]
        fn apply_inverts_the_fitted_line(
            g in -1.0f32..0.0, s_r in 0.8f32..1.2, o_r in -0.2f32..0.2,
            s_b in 0.8f32..1.2, o_b in -0.2f32..0.2
        ) {
            let cast = CastCorrection { slope_r: s_r, offset_r: o_r, slope_b: s_b, offset_b: o_b, cells_used: 8 };
            let e = [s_r * g + o_r, g, s_b * g + o_b];
            let out = apply_cast_pixel(e, &cast);
            prop_assert!((out[0] - g).abs() < 1e-5);
            prop_assert_eq!(out[1], g);
            prop_assert!((out[2] - g).abs() < 1e-5);
        }
    }
}
