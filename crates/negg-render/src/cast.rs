// SPDX-License-Identifier: MPL-2.0

//! Step 6 of `docs/MATHS.md`: auto colour (cast removal), per ADR 0009 as
//! amended and ADR 0027.
//!
//! The fit runs on the analysis grid after step 4, where every cell is a
//! normalised log exposure triple `e = (e_R, e_G, e_B)`, nominally in
//! `[-1, 0]`. After ADR 0011 the print is brighter where `e` is more
//! negative, so luma and hue are read on the print-space orientation
//! `p = -e` (ADR 0009 as amended by Ruling 30): a cell that prints green
//! has `p_G` largest. For a triple `e`:
//!
//! ```text
//! p      = -e
//! luma   l = (p_R + p_G + p_B) / 3
//! chroma c = max(p) - min(p)          (the same on e and on p)
//! hue      = HSV hue angle of (p_R, p_G, p_B): red 0, green 120, blue 240,
//!            undefined when c < 1e-4
//! ```
//!
//! The fitted model is one line per channel, on the uncorrected `e`:
//!
//! ```text
//! e_R = s_R * e_G + o_R
//! e_B = s_B * e_G + o_B
//! ```
//!
//! The cells it is fitted on are chosen in two stages (ADR 0027), because a
//! cast moves neutral cells in hue and in chroma, so neither can be read
//! reliably before the cast is known:
//!
//! 1. Provisional selection: the lowest-chroma 30% of all cells, with no
//!    luma bands and no hue sectors, read on the uncorrected `e`.
//! 2. Refinement, repeated at most 4 times: correct every cell with the
//!    current fit, read luma, chroma and hue on the corrected cell, split
//!    into shadows `[p0, p20)`, mids `[p20, p80)` and highlights
//!    `[p80, p100]` of the corrected luma, drop cells whose corrected hue
//!    lies in the foliage sector (90 to 160 degrees) or the sky sector (200
//!    to 260 degrees) and cells whose corrected chroma exceeds 0.1, keep
//!    the lowest-chroma 30% of the rest of each band, pool the bands and
//!    refit. It stops when the selection no longer changes, or keeps the
//!    previous fit when fewer than 8 cells survive.
//!
//! Each fit is ordinary least squares with the slope clamped to `[0.5, 2]`
//! (1 when the selected cells' `e_G` has no spread at all); the offset is
//! then taken through the centroid of the selected cells. The returned fit
//! reports a slope only when its own cells identify one: if their `e_G`
//! has a standard deviation below 0.03, its slopes are set to 1 and its
//! offsets refitted through the centroid. The provisional fit is not held
//! to that rule, because under a slope cast the lowest-chroma cells
//! cluster where the fitted lines cross neutral. Applying the
//! correction inverts the fitted lines so that the selected cells land on
//! `e_G`:
//!
//! ```text
//! e_R <- (e_R - o_R) / s_R
//! e_B <- (e_B - o_B) / s_B
//! ```
//!
//! Fewer than 8 cells in the provisional selection gives the identity and
//! `cells_used = 0`; otherwise `cells_used` is the size of the selection
//! the returned fit was made on.

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
/// Fraction of cells kept, lowest chroma first: of all cells in the
/// provisional selection, of each band's eligible cells in a refinement.
const LOW_CHROMA_FRACTION: f64 = 0.3;
/// Minimum cells for a fit; fewer gives the identity (provisional) or keeps
/// the previous fit (refinement).
const MIN_CELLS: usize = 8;
/// Bound of the fitted slopes: the validation range of a Fixed cast in
/// `negg-types`, so every fitted cast is a valid Fixed cast (ADR 0027).
const SLOPE_RANGE: (f64, f64) = (0.5, 2.0);
/// Below this standard deviation of `e_G` over the cells of the returned
/// fit its slopes are not identified and are set to 1 (ADR 0027).
/// Controller choice, `TODO(measure)`.
const MIN_G_SPREAD: f64 = 0.03;
/// In a refinement, a cell whose chroma after the current correction
/// exceeds this is scene colour, not a neutral, and is dropped (ADR 0027).
/// Controller choice, `TODO(measure)`.
const NEUTRAL_CHROMA_MAX: f32 = 0.1;
/// Most refinements after the provisional fit (ADR 0027). Controller
/// choice, `TODO(measure)`.
const MAX_REFINEMENTS: usize = 4;

/// Luma, chroma and hue of one triple, read on its print-space orientation.
#[derive(Clone, Copy, Debug)]
struct Cell {
    /// Mean of `-e`: higher prints brighter.
    luma: f32,
    chroma: f32,
    /// HSV hue of `-e` in degrees `[0, 360)`, `None` when chroma is below
    /// [`CHROMA_UNDEFINED`].
    hue: Option<f32>,
}

impl Cell {
    /// Luma, chroma and hue of the print-space triple `-e` (ADR 0009 as
    /// amended).
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
            luma: (r + g + b) / 3.0,
            chroma,
            hue,
        }
    }

    /// Whether the cell's hue allows it in the fit: undefined (neutral) or
    /// outside the foliage and sky sectors.
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

/// Sorts `(index, chroma)` pairs by chroma (stable, so ties keep index
/// order) and returns the indices of the lowest [`LOW_CHROMA_FRACTION`],
/// rounded up so a non-empty set keeps at least one.
fn keep_lowest_chroma(mut cells: Vec<(usize, f32)>) -> Vec<usize> {
    cells.sort_by(|a, b| a.1.total_cmp(&b.1));
    let keep = (cells.len() as f64 * LOW_CHROMA_FRACTION).ceil() as usize;
    cells.truncate(keep);
    cells.into_iter().map(|(i, _)| i).collect()
}

/// Stage 1 (ADR 0027): the lowest-chroma 30% of all cells, on the
/// uncorrected `e`, with no bands and no hue sectors. Sorted indices.
fn provisional_selection(es: &[[f32; 3]]) -> Vec<usize> {
    let cells = es
        .iter()
        .enumerate()
        .map(|(i, &e)| (i, Cell::from_e(e).chroma))
        .collect();
    let mut sel = keep_lowest_chroma(cells);
    sel.sort_unstable();
    sel
}

/// Stage 2 (ADR 0027): the cells chosen when luma, chroma and hue are read
/// after correcting by `cast`. Sorted indices.
fn refined_selection(es: &[[f32; 3]], cast: &CastCorrection) -> Vec<usize> {
    let cells: Vec<Cell> = es
        .iter()
        .map(|&e| Cell::from_e(apply_cast_pixel(e, cast)))
        .collect();
    let mut lumas: Vec<f32> = cells.iter().map(|c| c.luma).collect();
    lumas.sort_by(f32::total_cmp);
    let p20 = percentile_sorted(&lumas, SHADOW_PERCENTILE);
    let p80 = percentile_sorted(&lumas, HIGHLIGHT_PERCENTILE);

    let mut bands: [Vec<(usize, f32)>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for (i, cell) in cells.iter().enumerate() {
        if !cell.eligible() || cell.chroma > NEUTRAL_CHROMA_MAX {
            continue;
        }
        let band = if cell.luma < p20 {
            0
        } else if cell.luma < p80 {
            1
        } else {
            2
        };
        bands[band].push((i, cell.chroma));
    }
    let mut sel: Vec<usize> = bands.into_iter().flat_map(keep_lowest_chroma).collect();
    sel.sort_unstable();
    sel
}

/// Least squares of `y = s * x + o` over `(x, y)` pairs (ADR 0027). The
/// slope is `None` to fit it, or a value to hold it there; a fitted slope
/// is 1 when `x` has no spread at all and is clamped to [`SLOPE_RANGE`].
/// The offset is then taken through the centroid so the cells land on
/// `e_G` on average.
fn fit_line(xs: &[f64], ys: &[f64], held: Option<f64>) -> (f32, f32) {
    let n = xs.len() as f64;
    let mean_x = xs.iter().sum::<f64>() / n;
    let mean_y = ys.iter().sum::<f64>() / n;
    let slope = held.unwrap_or_else(|| {
        let mut sxx = 0.0;
        let mut sxy = 0.0;
        for (x, y) in xs.iter().zip(ys) {
            let dx = x - mean_x;
            sxx += dx * dx;
            sxy += dx * (y - mean_y);
        }
        let slope = if sxx > 0.0 { sxy / sxx } else { 1.0 };
        slope.clamp(SLOPE_RANGE.0, SLOPE_RANGE.1)
    });
    let offset = mean_y - slope * mean_x;
    (slope as f32, offset as f32)
}

/// Standard deviation of `e_G` over the cells `es[sel]`.
fn green_spread(es: &[[f32; 3]], sel: &[usize]) -> f64 {
    let n = sel.len() as f64;
    let mean = sel.iter().map(|&i| f64::from(es[i][1])).sum::<f64>() / n;
    let var = sel
        .iter()
        .map(|&i| (f64::from(es[i][1]) - mean).powi(2))
        .sum::<f64>()
        / n;
    var.sqrt()
}

/// Fits both lines on the uncorrected cells `es[sel]`, with the slopes
/// fitted, or held at 1 when `hold_slopes`.
fn fit_selection(es: &[[f32; 3]], sel: &[usize], hold_slopes: bool) -> CastCorrection {
    let channel = |c: usize| -> Vec<f64> { sel.iter().map(|&i| f64::from(es[i][c])).collect() };
    let g = channel(1);
    let held = hold_slopes.then_some(1.0);
    let (slope_r, offset_r) = fit_line(&g, &channel(0), held);
    let (slope_b, offset_b) = fit_line(&g, &channel(2), held);
    CastCorrection {
        slope_r,
        offset_r,
        slope_b,
        offset_b,
        cells_used: u32::try_from(sel.len()).unwrap_or(u32::MAX),
    }
}

/// Step 6 of `docs/MATHS.md` (ADR 0009 as amended, ADR 0027): fits the cast
/// correction on the analysis grid `e_grid` (the grid after step 4,
/// normalised log exposure).
///
/// See the module documentation for the two-stage selection and the fit.
/// Returns the identity with `cells_used = 0` when the provisional
/// selection has fewer than 8 cells; otherwise `cells_used` is the number
/// of cells the returned fit was made on. Cells with non-finite values are
/// ignored.
#[must_use]
pub fn fit_cast(e_grid: &Image) -> CastCorrection {
    let es: Vec<[f32; 3]> = e_grid
        .data()
        .as_chunks::<3>()
        .0
        .iter()
        .copied()
        .filter(|e| e.iter().all(|v| v.is_finite()))
        .collect();

    let mut sel = provisional_selection(&es);
    if sel.len() < MIN_CELLS {
        return CastCorrection::IDENTITY;
    }
    let mut cast = fit_selection(&es, &sel, false);
    for _ in 0..MAX_REFINEMENTS {
        let next = refined_selection(&es, &cast);
        if next.len() < MIN_CELLS || next == sel {
            break;
        }
        cast = fit_selection(&es, &next, false);
        sel = next;
    }
    // The returned fit reports a slope only when its own cells span enough
    // of e_G to identify one; otherwise the slopes are 1 and the offsets
    // the mean differences (ADR 0027).
    if green_spread(&es, &sel) < MIN_G_SPREAD {
        cast = fit_selection(&es, &sel, true);
    }
    cast
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
        green_ramp_in(n, -1.0, 0.0)
    }

    /// As [`green_ramp`], over `[lo, hi]`.
    fn green_ramp_in(n: usize, lo: f32, hi: f32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let j = (i * 7919) % n;
                hi - (hi - lo) * (j as f32) / (n as f32 - 1.0)
            })
            .collect()
    }

    /// Builds a `w` by `h` grid where `e_R = s_r * e_G + o_r` and
    /// `e_B = s_b * e_G + o_b`, with `e_G` over `[-1, 0]`.
    fn cast_grid(w: usize, h: usize, s_r: f32, o_r: f32, s_b: f32, o_b: f32) -> Image {
        cast_grid_in(w, h, (-1.0, 0.0), (s_r, o_r, s_b, o_b))
    }

    /// As [`cast_grid`], with `e_G` over `range` and the cast given as
    /// `(s_r, o_r, s_b, o_b)`.
    fn cast_grid_in(w: usize, h: usize, range: (f32, f32), cast: (f32, f32, f32, f32)) -> Image {
        let (s_r, o_r, s_b, o_b) = cast;
        let g = green_ramp_in(w * h, range.0, range.1);
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
    fn a_frame_of_one_colour_is_read_as_a_grey_scene_under_a_cast() {
        // ADR 0027: a frame that is one colour everywhere (all foliage, all
        // sky) cannot be told apart from a grey scene under a cast of that
        // colour, and the blue and green casts of the brief must be
        // removed, so such a frame is corrected to neutral. Before ADR 0027
        // both gave the identity.
        let mut foliage = Image::new(16, 16);
        for px in foliage.data_mut().as_chunks_mut::<3>().0.iter_mut() {
            // Green in print: e_G is the most negative channel.
            px.copy_from_slice(&[-0.3, -0.6, -0.3]);
        }
        let cast = fit_cast(&foliage);
        assert!(cast.cells_used >= 8, "{cast:?}");
        assert_neutral_after(&foliage, &cast, 1e-6);

        let mut sky = Image::new(16, 16);
        for (i, px) in sky.data_mut().as_chunks_mut::<3>().0.iter_mut().enumerate() {
            // Blue in print (hue 200 on -e): e_B is the most negative.
            let g = -(i as f32) / 256.0;
            px.copy_from_slice(&[g + 0.2, g, g - 0.1]);
        }
        let cast = fit_cast(&sky);
        assert_neutral_after(&sky, &cast, 1e-5);
    }

    #[test]
    fn sky_and_foliage_beside_a_minority_of_neutrals_are_still_excluded() {
        // ADR 0027: one cell in ten is a neutral grey; the rest are sky
        // (hue 220 on -e) and foliage (hue 140) whose saturation runs from
        // weak to strong; and the scene is seen through a magenta cast (hue
        // 300, outside both sectors). The greys are fewer than the 30% that
        // each band keeps, so the provisional fit and, without the sectors,
        // every refit take weak sky and foliage cells too and are pulled
        // towards them. Read after the provisional fit, the sectors must
        // remove them so that the refit lands on the greys (without them
        // the greys end up to 1.7e-3 off neutral).
        let (w, h) = (40, 30);
        let n = w * h;
        let g = green_ramp(n);
        let (o_r, o_b) = (-0.04f32, -0.04f32);
        let mut grid = Image::new(w, h);
        let mut grey = vec![false; n];
        for (i, px) in grid
            .data_mut()
            .as_chunks_mut::<3>()
            .0
            .iter_mut()
            .enumerate()
        {
            let eg = g[i];
            let a = 0.005 + 0.2 * ((i * 31) % 97) as f32 / 96.0;
            let scene = match i % 10 {
                0 => {
                    grey[i] = true;
                    [eg; 3]
                }
                // Sky in print: p_B largest, then p_G, then p_R.
                1..=4 => [eg + 0.5 * a, eg, eg - a],
                // Foliage in print: p_G largest, then p_B, then p_R.
                _ => [eg + a, eg, eg + 2.0 / 3.0 * a],
            };
            px.copy_from_slice(&[scene[0] + o_r, scene[1], scene[2] + o_b]);
        }
        // Read without the cast, the coloured cells are in their sectors.
        let unc = |x: usize| {
            let e = grid.pixel(x, 0);
            Cell::from_e([e[0] - o_r, e[1], e[2] - o_b])
        };
        assert!(
            !unc(1).eligible() && !unc(5).eligible(),
            "{:?} {:?}",
            unc(1),
            unc(5)
        );
        let cast = fit_cast(&grid);
        assert!(cast.cells_used >= 8, "{cast:?}");
        let mut fixed = grid.clone();
        apply_cast(&mut fixed, &cast);
        for (i, px) in fixed.data().as_chunks::<3>().0.iter().enumerate() {
            if grey[i] {
                assert!(
                    (px[0] - px[1]).abs() < 1e-4 && (px[2] - px[1]).abs() < 1e-4,
                    "grey {px:?} ({cast:?})"
                );
            }
        }
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
        // The provisional selection keeps ceil(19.2) = 20 of the 64 cells;
        // every cell shares one luma, so the refinement puts all 64 in one
        // band, keeps the same 20 and stops.
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
        // The provisional selection keeps 30% of all cells, rounded up: 24
        // cells keep 8, which is enough; 20 cells keep 6 and are not.
        let grid = cast_grid(5, 4, 0.9, 0.05, 1.0, 0.0);
        assert_eq!(fit_cast(&grid).cells_used, 0);
    }

    #[test]
    fn slopes_are_bounded_and_need_spread_in_green() {
        // ADR 0027: warm in print, outside both hue sectors, with slopes
        // just past the bound [0.5, 2] on both sides.
        let cast = fit_cast(&cast_grid(32, 24, 2.2, 0.0, 0.45, 0.1));
        assert_eq!(cast.slope_r, 2.0, "{cast:?}");
        assert_eq!(cast.slope_b, 0.5, "{cast:?}");
        assert!(cast.cells_used >= 8);
        // After the bound the offset goes through the centroid of the
        // selected cells: offset = mean_y - bounded_slope * mean_g. The
        // cells lie exactly on e_R = 2.2 e_G and e_B = 0.45 e_G + 0.1, so
        // offset_r = 0.2 mean_g and offset_b = 0.1 - 0.05 mean_g, which
        // ties the two offsets together.
        assert!(
            (cast.offset_b - (0.1 - 0.25 * cast.offset_r)).abs() < 1e-5,
            "{cast:?}"
        );
        // And the value itself, for this grid's selection.
        const OFFSET_R: f32 = -0.1142;
        assert!(
            (cast.offset_r - OFFSET_R).abs() < 1e-4,
            "offset_r {}",
            cast.offset_r
        );

        // When the cells of the returned fit spread in e_G by less than
        // 0.03 (standard deviation) the slope is not identified: it is 1
        // and the offset is the mean difference.
        let mut narrow = Image::new(16, 16);
        let mut sum_r = 0.0f64;
        for (i, px) in narrow
            .data_mut()
            .as_chunks_mut::<3>()
            .0
            .iter_mut()
            .enumerate()
        {
            let g = -0.5 - 0.05 * (i as f32) / 255.0;
            px.copy_from_slice(&[1.5 * g + 0.2, g, g]);
            sum_r += f64::from(px[0] - px[1]);
        }
        let cast = fit_cast(&narrow);
        assert_eq!(cast.slope_r, 1.0, "{cast:?}");
        assert_eq!(cast.slope_b, 1.0, "{cast:?}");
        // Every cell has the same chroma ordering, so the selection is a
        // band-balanced sample; its mean difference is near the grid's.
        assert!(
            (f64::from(cast.offset_r) - sum_r / 256.0).abs() < 0.01,
            "{cast:?}"
        );
    }

    #[test]
    fn slope_casts_are_removed_on_a_realistic_green_range() {
        // ADR 0027 (fix round 1): a grey scene whose e_G spans 0.5 or 0.3
        // of the normalised range, not the whole of [-1, 0], under slope
        // casts of 1.6 and 0.65. The lowest-chroma cells cluster where the
        // fitted lines cross neutral; the provisional fit must still
        // identify the slope from them.
        for range in [(-0.7, -0.2), (-0.6, -0.3)] {
            for cast in [
                (1.6, 0.0, 1.0, 0.0),
                (1.6, -0.02, 0.65, 0.03),
                (1.0, 0.0, 0.65, 0.0),
            ] {
                let grid = cast_grid_in(64, 48, range, cast);
                let fit = fit_cast(&grid);
                assert!(
                    (fit.slope_r - cast.0).abs() < 1e-3 && (fit.slope_b - cast.2).abs() < 1e-3,
                    "range {range:?} cast {cast:?}: {fit:?}"
                );
                assert_neutral_after(&grid, &fit, 1e-3);
            }
        }
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

    /// Asserts that every cell of `grid`, corrected by `cast`, is neutral
    /// within `tol`: `|e_R - e_G|` and `|e_B - e_G|` at most `tol`.
    fn assert_neutral_after(grid: &Image, cast: &CastCorrection, tol: f32) {
        let mut fixed = grid.clone();
        apply_cast(&mut fixed, cast);
        for px in fixed.data().as_chunks::<3>().0 {
            assert!(
                (px[0] - px[1]).abs() <= tol && (px[2] - px[1]).abs() <= tol,
                "{px:?} not neutral after {cast:?}"
            );
        }
    }

    #[test]
    fn blue_cast_on_a_grey_scene_is_removed() {
        // ADR 0027: a grey scene under a pure blue cast (e_B = e_G - 0.1,
        // so p_B is the largest channel and every cell has hue 240 on -e).
        // Classified on the uncorrected cells, all of them fall in the sky
        // sector and the fit is the identity.
        let grid = cast_grid(32, 24, 1.0, 0.0, 1.0, -0.1);
        assert_eq!(
            Cell::from_e(grid.pixel(3, 3)).hue.map(f32::round),
            Some(240.0)
        );
        let cast = fit_cast(&grid);
        assert!(cast.cells_used >= 8, "{cast:?}");
        assert_neutral_after(&grid, &cast, 1e-3);
    }

    #[test]
    fn green_cast_on_a_grey_scene_is_removed() {
        // ADR 0027: the same with a pure green cast (p_G largest, hue 120,
        // the foliage sector).
        let grid = cast_grid(32, 24, 1.0, 0.1, 1.0, 0.1);
        assert_eq!(
            Cell::from_e(grid.pixel(3, 3)).hue.map(f32::round),
            Some(120.0)
        );
        let cast = fit_cast(&grid);
        assert!(cast.cells_used >= 8, "{cast:?}");
        assert_neutral_after(&grid, &cast, 1e-3);
    }

    #[test]
    fn a_slope_cast_beyond_the_old_clamp_is_removed() {
        // ADR 0027: slopes of 1.6 and 0.65, well outside the old clamp of
        // [0.8, 1.2] and inside the new bound [0.5, 2].
        let grid = cast_grid(32, 24, 1.6, -0.02, 0.65, 0.03);
        let cast = fit_cast(&grid);
        assert!((cast.slope_r - 1.6).abs() < 1e-3, "{cast:?}");
        assert!((cast.slope_b - 0.65).abs() < 1e-3, "{cast:?}");
        assert_neutral_after(&grid, &cast, 1e-3);
    }

    /// HSV hue in degrees of the print-space triple of `e`.
    fn hue_of(e: [f32; 3]) -> f32 {
        Cell::from_e(e).hue.expect("chromatic cell")
    }

    #[test]
    fn a_warm_dominated_scene_is_not_pulled_grey() {
        // ADR 0027: 60% of the cells are one saturated warm colour (golden
        // rock: in print p_R > p_G > p_B, chroma 0.35) at mid luma, the
        // other 40% are neutral greys spread over the whole luma range,
        // and the scene is seen through a known cast that lies outside
        // both hue sectors. With the mids band almost all rock, the
        // lowest-chroma 30% of that band is mostly rock and the old fit
        // pulls the rock towards grey.
        let (w, h) = (40, 30);
        let n = w * h;
        let (s_r, o_r, s_b, o_b) = (1.1f32, -0.05f32, 0.9f32, -0.04f32);
        let mut scene = Image::new(w, h);
        let mut warm = vec![false; n];
        for (i, px) in scene
            .data_mut()
            .as_chunks_mut::<3>()
            .0
            .iter_mut()
            .enumerate()
        {
            let j = (i * 7919) % n;
            let u = j as f32 / (n as f32 - 1.0);
            if j % 5 < 3 {
                warm[i] = true;
                let g = -0.35 - 0.3 * u;
                px.copy_from_slice(&[g - 0.2, g, g + 0.15]);
            } else {
                px.copy_from_slice(&[-u; 3]);
            }
        }
        let mut seen = scene.clone();
        for px in seen.data_mut().as_chunks_mut::<3>().0 {
            px[0] = s_r * px[0] + o_r;
            px[2] = s_b * px[2] + o_b;
        }
        let cast = fit_cast(&seen);
        let mut fixed = seen.clone();
        apply_cast(&mut fixed, &cast);
        let got = fixed.data().as_chunks::<3>().0;
        let want = scene.data().as_chunks::<3>().0;
        for i in 0..n {
            if warm[i] {
                // The rock keeps its hue and its chroma.
                assert!(
                    (hue_of(got[i]) - hue_of(want[i])).abs() < 1.0,
                    "rock {:?} want {:?} ({cast:?})",
                    got[i],
                    want[i]
                );
                let chroma = |e: [f32; 3]| Cell::from_e(e).chroma;
                assert!((chroma(got[i]) - chroma(want[i])).abs() < 1e-3);
            } else {
                let px = got[i];
                assert!(
                    (px[0] - px[1]).abs() < 1e-3 && (px[2] - px[1]).abs() < 1e-3,
                    "grey {px:?} ({cast:?})"
                );
            }
        }
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
