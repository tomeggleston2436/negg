// SPDX-License-Identifier: MPL-2.0

//! The patch statistic of step 4 of `docs/MATHS.md` (R-patch, Ruling 43).

use negg_types::{Image, PatchRect};

use crate::stats::median;

/// Fraction of the rect's width and height dropped on each side before
/// selecting cells. Controller choice, `TODO(measure)` (ADR 0016).
const PATCH_SHRINK: f32 = 0.1;

/// Fewest cells a patch may be summarised from. Controller choice,
/// `TODO(measure)` (ADR 0016).
const PATCH_MIN_CELLS: usize = 4;

/// Per-channel median of the grid cells inside a patch, the one patch
/// statistic of `docs/MATHS.md` (step 4, "Patch statistic", R-patch,
/// Ruling 43; also used by the wedge and band fits of step 2).
///
/// `rect` is in fractions of the frame. It is shrunk by 10% of its width and
/// height on each side, and every grid cell whose centre
/// `((x + 0.5) / width, (y + 0.5) / height)` lies inside the shrunk rect
/// (edges included) contributes. Returns `None` when fewer than 4 cells
/// qualify or the rect is not finite. A median, not a mean, so a dust cell or
/// two does not move it (ground rule 5).
pub fn patch_median(grid_image: &Image, rect: PatchRect) -> Option<[f32; 3]> {
    let finite = rect.x.is_finite()
        && rect.y.is_finite()
        && rect.width.is_finite()
        && rect.height.is_finite();
    if !finite || rect.width <= 0.0 || rect.height <= 0.0 {
        return None;
    }
    let (gw, gh) = (grid_image.width(), grid_image.height());
    if gw == 0 || gh == 0 {
        return None;
    }
    let x0 = rect.x + PATCH_SHRINK * rect.width;
    let x1 = rect.x + (1.0 - PATCH_SHRINK) * rect.width;
    let y0 = rect.y + PATCH_SHRINK * rect.height;
    let y1 = rect.y + (1.0 - PATCH_SHRINK) * rect.height;

    let mut channels: [Vec<f32>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for y in 0..gh {
        let cy = (y as f32 + 0.5) / gh as f32;
        if cy < y0 || cy > y1 {
            continue;
        }
        for x in 0..gw {
            let cx = (x as f32 + 0.5) / gw as f32;
            if cx < x0 || cx > x1 {
                continue;
            }
            let p = grid_image.pixel(x, y);
            for c in 0..3 {
                channels[c].push(p[c]);
            }
        }
    }
    if channels[0].len() < PATCH_MIN_CELLS {
        return None;
    }
    Some([
        median(&mut channels[0]),
        median(&mut channels[1]),
        median(&mut channels[2]),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, width: f32, height: f32) -> PatchRect {
        PatchRect {
            x,
            y,
            width,
            height,
        }
    }

    fn constant(w: usize, h: usize, v: [f32; 3]) -> Image {
        let mut img = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.set_pixel(x, y, v);
            }
        }
        img
    }

    #[test]
    fn constant_grid_gives_the_constant() {
        let img = constant(20, 20, [0.1, 0.2, 0.3]);
        let m = patch_median(&img, rect(0.25, 0.25, 0.5, 0.5)).unwrap();
        assert_eq!(m, [0.1, 0.2, 0.3]);
    }

    #[test]
    fn only_the_patch_contributes() {
        let mut img = constant(20, 20, [1.0, 1.0, 1.0]);
        for y in 5..15 {
            for x in 5..15 {
                img.set_pixel(x, y, [0.5, 0.25, 0.125]);
            }
        }
        let m = patch_median(&img, rect(0.25, 0.25, 0.5, 0.5)).unwrap();
        assert_eq!(m, [0.5, 0.25, 0.125]);
    }

    #[test]
    fn dust_cells_do_not_move_it() {
        let mut img = constant(50, 50, [0.4, 0.4, 0.4]);
        // 2% of the 1600 cells inside the patch region, spread out.
        let mut n = 0;
        for y in 10..40 {
            for x in 10..40 {
                if (x * 7 + y * 13) % 50 == 0 && n < 18 {
                    img.set_pixel(x, y, [9.0, 0.0, 9.0]);
                    n += 1;
                }
            }
        }
        assert!(n > 0);
        let m = patch_median(&img, rect(0.2, 0.2, 0.6, 0.6)).unwrap();
        assert_eq!(m, [0.4, 0.4, 0.4]);
    }

    #[test]
    fn shrink_excludes_the_border_of_the_rect() {
        // 10x10 grid, rect covering the whole frame: shrink leaves centres in
        // [0.1, 0.9], so the outer ring (centres 0.05 and 0.95) is excluded.
        let mut img = constant(10, 10, [0.3, 0.3, 0.3]);
        for i in 0..10 {
            img.set_pixel(i, 0, [7.0, 7.0, 7.0]);
            img.set_pixel(i, 9, [7.0, 7.0, 7.0]);
            img.set_pixel(0, i, [7.0, 7.0, 7.0]);
            img.set_pixel(9, i, [7.0, 7.0, 7.0]);
        }
        let m = patch_median(&img, rect(0.0, 0.0, 1.0, 1.0)).unwrap();
        assert_eq!(m, [0.3, 0.3, 0.3]);
    }

    #[test]
    fn fewer_than_four_cells_is_none() {
        let img = constant(10, 10, [0.3; 3]);
        // Shrunk rect 0.08 wide holds at most one cell centre.
        assert!(patch_median(&img, rect(0.4, 0.4, 0.1, 0.1)).is_none());
        // Exactly 2x2 cells qualify: rect 0.5..0.9 shrunk to 0.54..0.86 has
        // centres 0.55, 0.65, 0.75, 0.85 (4 per axis, 16 cells).
        assert!(patch_median(&img, rect(0.5, 0.5, 0.4, 0.4)).is_some());
    }

    #[test]
    fn empty_or_bad_rect_is_none() {
        let img = constant(10, 10, [0.3; 3]);
        assert!(patch_median(&Image::new(0, 0), rect(0.0, 0.0, 1.0, 1.0)).is_none());
        assert!(patch_median(&img, rect(f32::NAN, 0.0, 1.0, 1.0)).is_none());
        assert!(patch_median(&img, rect(0.0, 0.0, 0.0, 1.0)).is_none());
    }
}
