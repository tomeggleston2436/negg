// SPDX-License-Identifier: MPL-2.0

//! Step 7 of `docs/MATHS.md`: the camera matrix applied to the positive.
//!
//! The matrix itself (camera RGB to linear Rec.2020 D65 via XYZ with a
//! Bradford adaptation) is built by `negg-cms`; this module only applies a
//! 3x3 to every pixel of `out_c` after inversion and the print stage, and
//! clips the result at 0. Ground rule 3 puts the per-pixel operation here.

use negg_types::Image;

use crate::par::for_each_pixel_mut;

/// Step 7 of `docs/MATHS.md` for one pixel: `m * [r, g, b]` with each
/// output channel clipped at 0 (row-major `m`: `out[i] = sum_j m[i][j] * in[j]`).
///
/// This is the kernel shared by [`apply_matrix`] and [`crate::render_full`].
#[inline]
#[must_use]
pub fn apply_matrix_pixel(rgb: [f32; 3], m: [[f32; 3]; 3]) -> [f32; 3] {
    let mut out = [0.0f32; 3];
    for (o, row) in out.iter_mut().zip(m.iter()) {
        *o = (row[0] * rgb[0] + row[1] * rgb[1] + row[2] * rgb[2]).max(0.0);
    }
    out
}

/// Step 7 of `docs/MATHS.md`: applies the 3x3 matrix `m` (camera RGB to
/// working space) to every pixel of `image` in place, clipping at 0. Runs
/// in parallel over row bands.
pub fn apply_matrix(image: &mut Image, m: [[f32; 3]; 3]) {
    for_each_pixel_mut(image, |px| apply_matrix_pixel(px, m));
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const IDENTITY: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

    #[test]
    fn identity_is_a_no_op_on_non_negative_input() {
        let mut img = Image::new(4, 3);
        for (i, v) in img.data_mut().iter_mut().enumerate() {
            *v = i as f32 * 0.01;
        }
        let before = img.clone();
        apply_matrix(&mut img, IDENTITY);
        assert_eq!(img, before);
    }

    #[test]
    fn rows_act_on_the_input_and_negatives_clip() {
        let m = [[0.5, 0.25, 0.25], [0.0, 1.0, 0.0], [1.0, -2.0, 0.0]];
        let out = apply_matrix_pixel([0.4, 0.8, 0.2], m);
        assert!((out[0] - 0.45).abs() < 1e-6);
        assert!((out[1] - 0.8).abs() < 1e-6);
        assert_eq!(out[2], 0.0);
    }

    #[test]
    fn image_matches_per_pixel() {
        let m = [[0.9, 0.1, 0.0], [0.05, 0.9, 0.05], [0.0, 0.2, 0.8]];
        let mut img = Image::new(6, 5);
        for (i, v) in img.data_mut().iter_mut().enumerate() {
            *v = ((i * 37) % 101) as f32 / 100.0;
        }
        let before = img.clone();
        apply_matrix(&mut img, m);
        for y in 0..5 {
            for x in 0..6 {
                assert_eq!(img.pixel(x, y), apply_matrix_pixel(before.pixel(x, y), m));
            }
        }
    }

    proptest! {
        /// Output is never negative, whatever the matrix.
        #[test]
        fn never_negative(
            r in 0.0f32..1.0, g in 0.0f32..1.0, b in 0.0f32..1.0,
            m in proptest::array::uniform3(proptest::array::uniform3(-2.0f32..2.0))
        ) {
            let out = apply_matrix_pixel([r, g, b], m);
            for o in out {
                prop_assert!(o >= 0.0);
            }
        }
    }
}
