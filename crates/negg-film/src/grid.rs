// SPDX-License-Identifier: MPL-2.0

//! The analysis grid of step 3 of `docs/MATHS.md` and the density
//! transform.
//!
//! The grid is a block-median downsample of the frame to about 1024 px on
//! the long side. Every statistic in steps 3, 5 and 6 is computed on it,
//! never on the full-resolution image (ground rule 5): a block median
//! throws away isolated dust, scratches and hot pixels before any
//! percentile is taken.

use negg_types::Image;

use crate::stats::median;

/// The block-median analysis grid of step 3 of `docs/MATHS.md`
/// (`docs/MATHS.md#step-3-density-and-film-base`).
///
/// `median` holds one pixel per block, the per-channel median of the
/// block's linear camera RGB. `cv` holds, per cell in the same row-major
/// order, the coefficient of variation (population standard deviation over
/// mean) of the block's luma, where a pixel's luma is the mean of its three
/// linear channels. A cell with `cv < 0.03` is "flat" (step 3) and one
/// with `cv >= 0.03` is "textured" (step 5).
#[derive(Clone, Debug, PartialEq)]
pub struct AnalysisGrid {
    /// Block medians of the linear camera RGB, one pixel per cell.
    pub median: Image,
    /// Coefficient of variation of each block's luma, `len = width * height`
    /// of `median`. Zero when the block's mean luma is zero.
    pub cv: Vec<f32>,
    /// Side of the square block each cell summarises, in source pixels.
    /// Blocks on the right and bottom edges may be smaller.
    pub block: usize,
    /// Width of the source image the grid was built from, in pixels. Kept
    /// by [`sub_grid`], so cell `i` of the original grid covers source
    /// columns `[i b, min((i + 1) b, source_width))`.
    pub source_width: usize,
    /// Height of the source image the grid was built from, in pixels.
    pub source_height: usize,
}

impl AnalysisGrid {
    /// Width of the grid in cells.
    pub fn width(&self) -> usize {
        self.median.width()
    }

    /// Height of the grid in cells.
    pub fn height(&self) -> usize {
        self.median.height()
    }

    /// Number of cells, `width * height`.
    pub fn cells(&self) -> usize {
        self.cv.len()
    }
}

/// Builds the analysis grid of step 3 of `docs/MATHS.md`
/// (`docs/MATHS.md#step-3-density-and-film-base`) from a linear camera RGB
/// image.
///
/// The block size is `b = ceil(long_side_px / long_side)`, at least 1, so
/// the grid is at most `long_side` cells on its long side (the plan's
/// "about 1024 px"). A 2049 px wide frame therefore gets `b = 3` and 683
/// columns, and a frame no larger than `long_side` is its own grid with
/// every `cv` zero. Each cell is the per-channel median of its `b x b`
/// block (smaller on the right and bottom edges); see [`AnalysisGrid`] for
/// `cv`.
///
/// Cost: each block is a small sort per channel. A 24 MP frame (6000 px
/// wide) has `b = 6`, so about 670 thousand blocks of 36 pixels, which is
/// well under a second single-threaded; this crate does not use rayon.
///
/// `long_side` of 0 is treated as 1. An image with a zero dimension gives
/// an empty grid.
pub fn analysis_grid(image: &Image, long_side: usize) -> AnalysisGrid {
    let long_side = long_side.max(1);
    let block = image.width().max(image.height()).div_ceil(long_side).max(1);
    let grid_w = image.width().div_ceil(block);
    let grid_h = image.height().div_ceil(block);
    let (grid_w, grid_h) = if image.width() == 0 || image.height() == 0 {
        (0, 0)
    } else {
        (grid_w, grid_h)
    };

    let mut median_img = Image::new(grid_w, grid_h);
    let mut cv = Vec::with_capacity(grid_w * grid_h);
    let mut r = Vec::with_capacity(block * block);
    let mut g = Vec::with_capacity(block * block);
    let mut b = Vec::with_capacity(block * block);

    // `tiles` walks row-major, which matches the grid's pixel order.
    for (i, tile) in image.tiles(block).enumerate() {
        r.clear();
        g.clear();
        b.clear();
        // Accumulated in f64: a 36-pixel block is small, but a 1x1 grid of a
        // whole frame is not, and the variance of nearly equal values is
        // fragile in f32.
        let mut sum = 0.0f64;
        let mut sum_sq = 0.0f64;
        for [pr, pg, pb] in image.tile_pixels(tile) {
            r.push(pr);
            g.push(pg);
            b.push(pb);
            let luma = (f64::from(pr) + f64::from(pg) + f64::from(pb)) / 3.0;
            sum += luma;
            sum_sq += luma * luma;
        }
        let n = r.len() as f64;
        let mean = sum / n;
        let var = (sum_sq / n - mean * mean).max(0.0);
        let cell_cv = if mean > 0.0 { var.sqrt() / mean } else { 0.0 };
        cv.push(cell_cv as f32);

        let x = i % grid_w;
        let y = i / grid_w;
        median_img.set_pixel(x, y, [median(&mut r), median(&mut g), median(&mut b)]);
    }

    AnalysisGrid {
        median: median_img,
        cv,
        block,
        source_width: image.width(),
        source_height: image.height(),
    }
}

/// The cells of one axis whose centres lie in `[p0, p1)`: cell `i` of a
/// `cells`-long axis with block `b` over `n` source pixels has its centre
/// at `(i b + min((i + 1) b, n)) / 2`. Compared doubled, in integers.
fn axis_cells(cells: usize, b: usize, n: usize, p0: usize, p1: usize) -> (usize, usize) {
    let twice_centre = |i: usize| i * b + ((i + 1) * b).min(n);
    let first = (0..cells)
        .find(|&i| twice_centre(i) >= 2 * p0)
        .unwrap_or(cells);
    let end = (first..cells)
        .find(|&i| twice_centre(i) >= 2 * p1)
        .unwrap_or(cells);
    (first, end)
}

/// The analysis area of step 3 of `docs/MATHS.md`
/// (`docs/MATHS.md#step-3-density-and-film-base`, Ruling 90): the cell
/// rectangle `(cx0, cy0, cx1, cy1)`, ends exclusive, of the cells of `grid`
/// whose centres in source pixels lie inside the pixel rectangle
/// `rect = (x0, y0, x1, y1)` (ends exclusive). A cell's centre is
/// `(i b + min((i + 1) b, W)) / 2` across and the same down, with `b` the
/// block and `W` the source width. The result may be empty (`cx0 == cx1`
/// or `cy0 == cy1`) when no centre falls inside.
pub fn grid_cells_in(
    grid: &AnalysisGrid,
    rect: (usize, usize, usize, usize),
) -> (usize, usize, usize, usize) {
    let (x0, y0, x1, y1) = rect;
    let b = grid.block.max(1);
    let (cx0, cx1) = axis_cells(grid.width(), b, grid.source_width, x0, x1);
    let (cy0, cy1) = axis_cells(grid.height(), b, grid.source_height, y0, y1);
    (cx0, cy0, cx1, cy1)
}

/// The part of `grid` inside the cell rectangle
/// `cells = (cx0, cy0, cx1, cy1)` (ends exclusive, clamped to the grid), as
/// a grid of its own for the estimators of step 3 of `docs/MATHS.md`
/// (`docs/MATHS.md#step-3-density-and-film-base`, Ruling 90): `median` and
/// `cv` cropped, `block` and the source size kept. An empty rectangle gives
/// a grid with no cells.
pub fn sub_grid(grid: &AnalysisGrid, cells: (usize, usize, usize, usize)) -> AnalysisGrid {
    let (w, h) = (grid.width(), grid.height());
    let cx1 = cells.2.min(w);
    let cy1 = cells.3.min(h);
    let cx0 = cells.0.min(cx1);
    let cy0 = cells.1.min(cy1);
    let (sw, sh) = (cx1 - cx0, cy1 - cy0);
    let (median, cv) = if sw == 0 || sh == 0 {
        (Image::new(0, 0), Vec::new())
    } else {
        let median = grid
            .median
            .crop(cx0, cy0, sw, sh)
            .expect("the cell rectangle is clamped to the grid");
        let mut cv = Vec::with_capacity(sw * sh);
        for y in cy0..cy1 {
            cv.extend_from_slice(&grid.cv[y * w + cx0..y * w + cx1]);
        }
        (median, cv)
    };
    AnalysisGrid {
        median,
        cv,
        block: grid.block,
        source_width: grid.source_width,
        source_height: grid.source_height,
    }
}

/// The transmittance floor of step 3 of `docs/MATHS.md`: a sample below
/// it is read as `1e-6`, so its density is finite at 6.0.
pub const TRANSMITTANCE_FLOOR: f32 = 1e-6;

/// Density of one linear sample, step 3 of `docs/MATHS.md`
/// (`docs/MATHS.md#step-3-density-and-film-base`):
/// `D = -log10(max(I, 1e-6))`.
///
/// The one copy of the formula: [`density`] maps it over an image, and
/// `negg-render`'s fused full-resolution render calls it per pixel so that
/// no full-size density image is allocated.
#[inline]
pub fn density_value(i: f32) -> f32 {
    -(i.max(TRANSMITTANCE_FLOOR)).log10()
}

/// Density per channel, step 3 of `docs/MATHS.md`
/// (`docs/MATHS.md#step-3-density-and-film-base`):
/// `D_c = -log10(max(I_c, 1e-6))`, with [`density_value`] per sample.
///
/// Pure and shape-preserving. The pipeline calls it on the analysis grid;
/// the full-resolution frame takes the density inside `negg-render`'s
/// fused render instead. The floor of `1e-6` keeps the density finite, at
/// 6.0, for a zero sample.
pub fn density(linear: &Image) -> Image {
    let data: Vec<f32> = linear.data().iter().map(|&i| density_value(i)).collect();
    Image::from_vec(linear.width(), linear.height(), data)
        .expect("density preserves the buffer length")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_size_and_columns_for_2049_wide() {
        let img = Image::new(2049, 10);
        let grid = analysis_grid(&img, 1024);
        assert_eq!(grid.block, 3);
        assert_eq!(grid.width(), 683);
        assert_eq!(grid.height(), 4);
        assert_eq!(grid.cells(), 683 * 4);
        assert_eq!(grid.cv.len(), grid.median.data().len() / 3);
    }

    #[test]
    fn small_image_is_its_own_grid() {
        let mut img = Image::new(5, 4);
        img.set_pixel(2, 3, [0.1, 0.2, 0.3]);
        let grid = analysis_grid(&img, 1024);
        assert_eq!(grid.block, 1);
        assert_eq!(grid.width(), 5);
        assert_eq!(grid.height(), 4);
        assert_eq!(grid.median.pixel(2, 3), [0.1, 0.2, 0.3]);
        assert!(grid.cv.iter().all(|&c| c == 0.0));
    }

    #[test]
    fn exact_long_side_gives_block_one() {
        let img = Image::new(1024, 700);
        assert_eq!(analysis_grid(&img, 1024).block, 1);
        let img = Image::new(1025, 700);
        assert_eq!(analysis_grid(&img, 1024).block, 2);
        let img = Image::new(700, 2048);
        let grid = analysis_grid(&img, 1024);
        assert_eq!(grid.block, 2);
        assert_eq!((grid.width(), grid.height()), (350, 1024));
    }

    #[test]
    fn empty_image_gives_empty_grid() {
        let grid = analysis_grid(&Image::new(0, 7), 1024);
        assert_eq!(grid.cells(), 0);
        assert_eq!(grid.width(), 0);
        let grid = analysis_grid(&Image::new(7, 0), 0);
        assert_eq!(grid.cells(), 0);
    }

    #[test]
    fn block_median_ignores_a_single_outlier_and_measures_texture() {
        // One 4x4 block: 15 pixels at 0.5 and one dust pixel at 0.001.
        let mut img = Image::new(4, 4);
        for p in img.data_mut().iter_mut() {
            *p = 0.5;
        }
        img.set_pixel(1, 2, [0.001; 3]);
        let grid = analysis_grid(&img, 1);
        assert_eq!(grid.block, 4);
        assert_eq!(grid.cells(), 1);
        assert_eq!(grid.median.pixel(0, 0), [0.5; 3]);
        // The dust pixel makes the block textured but only slightly.
        assert!(grid.cv[0] > 0.0 && grid.cv[0] < 0.3, "{}", grid.cv[0]);

        // A perfectly flat block has cv 0; an all-black block has cv 0 too.
        let flat = analysis_grid(&Image::from_vec(2, 2, vec![0.25; 12]).unwrap(), 1);
        assert_eq!(flat.cv, vec![0.0]);
        let black = analysis_grid(&Image::new(2, 2), 1);
        assert_eq!(black.cv, vec![0.0]);
    }

    #[test]
    fn cv_is_std_over_mean_of_luma() {
        // Luma per pixel: 1, 3 -> mean 2, population std 1 -> cv 0.5.
        let img = Image::from_vec(2, 1, vec![1.0, 1.0, 1.0, 3.0, 3.0, 3.0]).unwrap();
        let grid = analysis_grid(&img, 1);
        assert!((grid.cv[0] - 0.5).abs() < 1e-6, "{}", grid.cv[0]);
        // Even count: the median is the mean of the two middle values.
        assert_eq!(grid.median.pixel(0, 0), [2.0; 3]);
    }

    #[test]
    fn edge_blocks_are_smaller_and_still_medians() {
        // 5x3 with block 2 -> 3x2 grid; the right column is 1 px wide.
        let mut img = Image::new(5, 3);
        for y in 0..3 {
            for x in 0..5 {
                let v = (y * 5 + x) as f32;
                img.set_pixel(x, y, [v, v, v]);
            }
        }
        let grid = analysis_grid(&img, 3);
        assert_eq!(grid.block, 2);
        assert_eq!((grid.width(), grid.height()), (3, 2));
        // Block (0,0) covers values 0, 1, 5, 6 -> median 3.
        assert_eq!(grid.median.pixel(0, 0), [3.0; 3]);
        // Block (2,0) covers values 4, 9 -> median 6.5.
        assert_eq!(grid.median.pixel(2, 0), [6.5; 3]);
        // Block (2,1) covers value 14 only.
        assert_eq!(grid.median.pixel(2, 1), [14.0; 3]);
    }

    #[test]
    fn the_grid_records_its_source_size() {
        let grid = analysis_grid(&Image::new(2049, 10), 1024);
        assert_eq!((grid.source_width, grid.source_height), (2049, 10));
    }

    #[test]
    fn cells_in_a_rect_are_those_whose_centres_lie_inside() {
        // 10 x 7 source, block 3: columns cover [0,3) [3,6) [6,9) [9,10),
        // centres 1.5, 4.5, 7.5, 9.5; rows [0,3) [3,6) [6,7), centres 1.5,
        // 4.5, 6.5.
        let grid = analysis_grid(&Image::new(10, 7), 4);
        assert_eq!(grid.block, 3);
        assert_eq!((grid.width(), grid.height()), (4, 3));
        assert_eq!(grid_cells_in(&grid, (0, 0, 10, 7)), (0, 0, 4, 3));
        assert_eq!(grid_cells_in(&grid, (2, 2, 8, 5)), (1, 1, 3, 2));
        let grid1 = analysis_grid(&Image::new(8, 8), 4);
        assert_eq!(grid1.block, 2);
        // Centres 1, 3, 5, 7: one on the left edge is inside, one on the
        // right edge is not.
        assert_eq!(grid_cells_in(&grid1, (3, 0, 7, 8)), (1, 0, 3, 4));
        // Centres 1, 3, 5, 7: [1, 5) holds the first two.
        assert_eq!(grid_cells_in(&grid1, (1, 1, 5, 5)), (0, 0, 2, 2));
        // No centre inside: empty.
        let (cx0, _, cx1, _) = grid_cells_in(&grid1, (2, 0, 3, 8));
        assert_eq!(cx0, cx1);
    }

    #[test]
    fn sub_grid_crops_median_and_cv_and_keeps_block_and_source() {
        let mut img = Image::new(6, 4);
        for y in 0..4 {
            for x in 0..6 {
                let v = (y * 6 + x) as f32 + 1.0;
                img.set_pixel(x, y, [v, v, v]);
            }
        }
        let mut grid = analysis_grid(&img, 6);
        assert_eq!(grid.block, 1);
        for (i, cv) in grid.cv.iter_mut().enumerate() {
            *cv = i as f32;
        }
        let sub = sub_grid(&grid, (1, 2, 4, 4));
        assert_eq!((sub.width(), sub.height()), (3, 2));
        assert_eq!(sub.median.pixel(0, 0), grid.median.pixel(1, 2));
        assert_eq!(sub.median.pixel(2, 1), grid.median.pixel(3, 3));
        assert_eq!(sub.cv, vec![13.0, 14.0, 15.0, 19.0, 20.0, 21.0]);
        assert_eq!(sub.block, 1);
        assert_eq!((sub.source_width, sub.source_height), (6, 4));
        assert_eq!(sub_grid(&grid, (0, 0, 6, 4)), grid);
        assert_eq!(sub_grid(&grid, (2, 0, 2, 4)).cells(), 0);
        assert_eq!(sub_grid(&grid, (0, 0, 99, 99)), grid);
    }

    #[test]
    fn density_is_minus_log10_with_a_floor() {
        let img = Image::from_vec(2, 1, vec![1.0, 0.1, 0.01, 0.0, -1.0, 1e-7]).unwrap();
        let d = density(&img);
        assert_eq!((d.width(), d.height()), (2, 1));
        let got = d.data();
        assert!((got[0] - 0.0).abs() < 1e-6);
        assert!((got[1] - 1.0).abs() < 1e-6);
        assert!((got[2] - 2.0).abs() < 1e-5);
        assert!((got[3] - 6.0).abs() < 1e-5, "zero is floored to 1e-6");
        assert!((got[4] - 6.0).abs() < 1e-5, "negatives are floored too");
        assert!((got[5] - 6.0).abs() < 1e-5);
    }
}
