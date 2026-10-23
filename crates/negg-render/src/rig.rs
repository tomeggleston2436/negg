// SPDX-License-Identifier: MPL-2.0

//! Step 2 of `docs/MATHS.md` applied at render time, and the step 3
//! density with a rig density model.
//!
//! [`apply_rig_linear`] runs the linear part of a rig correction on the
//! step 1 output, in the order of ADR 0014 (R-rig-order): the flat field
//! gain (R-flat, bilinear between grid cell centres), then the crosstalk
//! unmix `I <- max(M^-1 I, 0)` (R-crosstalk). The density model of
//! R-density-use is not applied to the image: it replaces the density
//! formula of step 3, through [`density_model_pixel`] and [`density_grid`]
//! on the analysis grid and per pixel in [`crate::render_full`].
//!
//! The gains and matrices themselves are computed by `negg-calib`; this
//! module only applies them.

use negg_film::{density, density_value};
use negg_types::image::CHANNELS;
use negg_types::{DensityModel, Error, FlatField, Image, Result, RigCorrection};

use crate::par::{for_each_pixel_mut, for_each_row_mut};

/// Step 3 of `docs/MATHS.md` with a rig density model (step 2,
/// R-density-use, ADR 0015): `D_c = a_c + sum_k g[c][k] * log10(max(I_k,
/// 1e-6))`.
///
/// The floored log is `-negg_film::density_value(I_k)`, so the floor is the
/// step 3 one. With [`DensityModel::IDENTITY`] this equals
/// `negg_film::density_value` per channel.
#[inline]
#[must_use]
pub fn density_model_pixel(i: [f32; 3], m: &DensityModel) -> [f32; 3] {
    let l = [
        -density_value(i[0]),
        -density_value(i[1]),
        -density_value(i[2]),
    ];
    std::array::from_fn(|c| m.a[c] + (m.g[c][0] * l[0] + m.g[c][1] * l[1] + m.g[c][2] * l[2]))
}

/// Step 3 of `docs/MATHS.md` on an image (normally the analysis grid's
/// block medians): the density per pixel, from the rig density model when
/// one is given ([`density_model_pixel`], R-density-use) and otherwise
/// `negg_film::density` (`D_c = -log10(max(I_c, 1e-6))`).
///
/// Shape-preserving; runs in parallel over row bands with a model.
#[must_use]
pub fn density_grid(linear: &Image, model: Option<&DensityModel>) -> Image {
    match model {
        None => density(linear),
        Some(m) => {
            let mut out = linear.clone();
            for_each_pixel_mut(&mut out, |i| density_model_pixel(i, m));
            out
        }
    }
}

/// One axis of the R-flat bilinear lookup: for each output position, the
/// two grid indices and the weight of the second.
fn axis_weights(len: usize, grid_len: usize) -> Vec<(usize, usize, f32)> {
    let last = grid_len - 1;
    (0..len)
        .map(|p| {
            let u = ((p as f64 + 0.5) / len as f64 * grid_len as f64 - 0.5).clamp(0.0, last as f64);
            let i0 = u.floor() as usize;
            let i1 = (i0 + 1).min(last);
            (i0, i1, (u - i0 as f64) as f32)
        })
        .collect()
}

/// `a + (b - a) * t`: exact at `a == b`, so a uniform gain of 1 stays 1.
#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Checks a flat field against the frame it is applied to.
fn check_flat(flat: &FlatField, image: &Image) -> Result<()> {
    if flat.width != image.width() || flat.height != image.height() {
        return Err(Error::Invalid(format!(
            "flat field is for a {}x{} frame but the frame is {}x{}",
            flat.width,
            flat.height,
            image.width(),
            image.height()
        )));
    }
    if flat.gain.width() == 0 || flat.gain.height() == 0 {
        return Err(Error::Invalid(format!(
            "flat field gain grid is {}x{}: it has no cells",
            flat.gain.width(),
            flat.gain.height()
        )));
    }
    if !flat.gain.data().iter().all(|g| g.is_finite() && *g > 0.0) {
        return Err(Error::Invalid(
            "flat field gains must be finite and positive".to_string(),
        ));
    }
    Ok(())
}

/// Multiplies `image` by the flat field gain, bilinear between grid cell
/// centres (R-flat). Interpolates the two grid rows into one gain line per
/// image row, then each pixel between two entries of that line.
fn apply_flat(image: &mut Image, flat: &FlatField) {
    let gain = &flat.gain;
    let (gw, gh) = (gain.width(), gain.height());
    let xs = axis_weights(image.width(), gw);
    let ys = axis_weights(image.height(), gh);
    let g = gain.data();
    for_each_row_mut(image, |y, row| {
        let (y0, y1, fy) = ys[y];
        let line: Vec<f32> = (0..gw * CHANNELS)
            .map(|i| lerp(g[y0 * gw * CHANNELS + i], g[y1 * gw * CHANNELS + i], fy))
            .collect();
        for (px, &(x0, x1, fx)) in row.as_chunks_mut::<CHANNELS>().0.iter_mut().zip(&xs) {
            for c in 0..CHANNELS {
                px[c] *= lerp(line[x0 * CHANNELS + c], line[x1 * CHANNELS + c], fx);
            }
        }
    });
}

/// Step 2 of `docs/MATHS.md` on the step 1 output, in place, in the order
/// of R-rig-order (ADR 0014): the flat field gain when `rig.flat` is set
/// (R-flat: per pixel, bilinear between grid cell centres at
/// `u = (x + 0.5) / W * gw - 0.5` clamped to `[0, gw - 1]`, the same for
/// `v`), then the unmix `I <- max(M^-1 I, 0)` when `rig.unmix` is set
/// (R-crosstalk). `rig.density` is not used here: it replaces the step 3
/// density (see [`density_grid`] and [`crate::render_full`]). Runs in
/// parallel over row bands.
///
/// # Errors
///
/// [`Error::Invalid`] when the flat field is for a different frame size
/// than `image`, its gain grid is empty, or a gain is not finite and
/// positive, or when the unmix matrix has a non-finite entry. The image is
/// untouched on error.
pub fn apply_rig_linear(image: &mut Image, rig: &RigCorrection) -> Result<()> {
    if let Some(flat) = &rig.flat {
        check_flat(flat, image)?;
    }
    if let Some(m) = &rig.unmix
        && !m.iter().flatten().all(|v| v.is_finite())
    {
        return Err(Error::Invalid(format!(
            "rig {} unmix matrix {m:?} must be finite",
            rig.name
        )));
    }
    if let Some(flat) = &rig.flat {
        apply_flat(image, flat);
    }
    if let Some(m) = rig.unmix {
        for_each_pixel_mut(image, |i| {
            let mut out = [0.0f32; 3];
            for c in 0..3 {
                out[c] = (m[c][0] * i[0] + m[c][1] * i[1] + m[c][2] * i[2]).max(0.0);
            }
            out
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDENTITY3: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

    fn ramp(w: usize, h: usize) -> Image {
        let mut img = Image::new(w, h);
        for (i, v) in img.data_mut().iter_mut().enumerate() {
            *v = 0.001 + (i % 997) as f32 / 1000.0;
        }
        img
    }

    fn rig(flat: Option<FlatField>, unmix: Option<[[f32; 3]; 3]>) -> RigCorrection {
        RigCorrection {
            name: "test rig".to_string(),
            flat,
            unmix,
            density: None,
        }
    }

    fn uniform_flat(w: usize, h: usize, gw: usize, gh: usize, gain: f32) -> FlatField {
        let mut g = Image::new(gw, gh);
        for v in g.data_mut() {
            *v = gain;
        }
        FlatField {
            gain: g,
            width: w,
            height: h,
        }
    }

    #[test]
    fn identity_density_model_equals_density() {
        let img = ramp(37, 11);
        let mut with_zero = img.clone();
        with_zero.set_pixel(0, 0, [0.0, 1.0, 1e-9]);
        for im in [&img, &with_zero] {
            let plain = density(im);
            let model = density_grid(im, Some(&DensityModel::IDENTITY));
            assert_eq!(plain, model);
            assert_eq!(density_grid(im, None), plain);
        }
        for i in [[0.5, 0.25, 0.125], [1e-7, 2.0, 1.0]] {
            let d = density_model_pixel(i, &DensityModel::IDENTITY);
            for c in 0..3 {
                assert_eq!(d[c], density_value(i[c]));
            }
        }
    }

    #[test]
    fn density_model_applies_offset_and_cross_terms() {
        let m = DensityModel {
            a: [0.1, 0.2, 0.3],
            g: [[-1.1, 0.05, 0.0], [0.02, -0.9, 0.01], [0.0, 0.03, -1.2]],
        };
        let i = [0.1f32, 0.01, 1e-8];
        let l = [-1.0f32, -2.0, -6.0];
        let d = density_model_pixel(i, &m);
        for c in 0..3 {
            let want = m.a[c] + m.g[c][0] * l[0] + m.g[c][1] * l[1] + m.g[c][2] * l[2];
            assert!((d[c] - want).abs() < 1e-5, "{d:?}");
        }
    }

    #[test]
    fn uniform_flat_and_identity_unmix_leave_pixels_unchanged() {
        let img = ramp(203, 77);
        let mut out = img.clone();
        apply_rig_linear(
            &mut out,
            &rig(Some(uniform_flat(203, 77, 9, 4, 1.0)), Some(IDENTITY3)),
        )
        .unwrap();
        assert_eq!(out, img);
        // No parts at all is the identity too.
        apply_rig_linear(&mut out, &rig(None, None)).unwrap();
        assert_eq!(out, img);
    }

    #[test]
    fn flat_gain_is_bilinear_between_cell_centres() {
        // A 2x1 grid over a 4x2 frame: cell centres sit at x = 1 and x = 3
        // in pixel units (u = 0 at pixel 0.5 and 1.5, u = 1 at 2.5 and 3.5).
        let mut gain = Image::new(2, 1);
        gain.set_pixel(0, 0, [1.0, 2.0, 0.5]);
        gain.set_pixel(1, 0, [3.0, 2.0, 1.5]);
        let flat = FlatField {
            gain,
            width: 4,
            height: 2,
        };
        let mut img = Image::new(4, 2);
        for v in img.data_mut() {
            *v = 1.0;
        }
        apply_rig_linear(&mut img, &rig(Some(flat), None)).unwrap();
        // u = (x + 0.5) / 4 * 2 - 0.5 = -0.25, 0.25, 0.75, 1.25, clamped.
        let want_r = [1.0, 1.5, 2.5, 3.0];
        for y in 0..2 {
            for (x, w) in want_r.iter().enumerate() {
                let px = img.pixel(x, y);
                assert!((px[0] - w).abs() < 1e-6, "x {x}: {px:?}");
                assert_eq!(px[1], 2.0);
                let wb = 0.5 + (w - 1.0) / 2.0;
                assert!((px[2] - wb).abs() < 1e-6, "x {x}: {px:?}");
            }
        }
    }

    #[test]
    fn flat_corrects_a_vignette_at_cell_centres() {
        // A frame of 3x3 blocks, each a constant, and a gain grid that is
        // the reciprocal: every block's centre pixel lies on its cell
        // centre (u = (3k + 1.5) / W * gw - 0.5 = k), so it comes out 1.
        let (gw, gh, b) = (8, 5, 3);
        let (w, h) = (gw * b, gh * b);
        let mut gain = Image::new(gw, gh);
        let mut img = Image::new(w, h);
        for gy in 0..gh {
            for gx in 0..gw {
                let v = 0.5 + 0.05 * (gx + 2 * gy) as f32;
                gain.set_pixel(gx, gy, [1.0 / v, 0.5 / v, 2.0 / v]);
                for y in gy * b..(gy + 1) * b {
                    for x in gx * b..(gx + 1) * b {
                        img.set_pixel(x, y, [v; 3]);
                    }
                }
            }
        }
        let flat = FlatField {
            gain,
            width: w,
            height: h,
        };
        apply_rig_linear(&mut img, &rig(Some(flat), None)).unwrap();
        for gy in 0..gh {
            for gx in 0..gw {
                let px = img.pixel(gx * b + 1, gy * b + 1);
                for (c, want) in [1.0f32, 0.5, 2.0].iter().enumerate() {
                    assert!((px[c] - want).abs() < 1e-5, "cell {gx},{gy}: {px:?}");
                }
            }
        }
    }

    #[test]
    fn unmix_mixes_and_clips_at_zero() {
        let m = [[1.2, -0.3, 0.0], [-0.1, 1.1, -0.05], [0.0, -0.2, 1.3]];
        let mut img = Image::new(2, 1);
        img.set_pixel(0, 0, [0.5, 0.4, 0.3]);
        img.set_pixel(1, 0, [0.0, 1.0, 0.0]);
        apply_rig_linear(&mut img, &rig(None, Some(m))).unwrap();
        let p = img.pixel(0, 0);
        let want = [0.6 - 0.12, -0.05 + 0.44 - 0.015, -0.08 + 0.39];
        for c in 0..3 {
            assert!((p[c] - want[c]).abs() < 1e-6, "{p:?}");
        }
        // Negative results clip at 0.
        assert_eq!(img.pixel(1, 0), [0.0, 1.1, 0.0]);
    }

    #[test]
    fn flat_runs_before_unmix() {
        let m = [[1.0, -0.5, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let mut gain = Image::new(1, 1);
        gain.set_pixel(0, 0, [1.0, 2.0, 1.0]);
        let flat = FlatField {
            gain,
            width: 1,
            height: 1,
        };
        let mut img = Image::new(1, 1);
        img.set_pixel(0, 0, [1.0, 0.5, 1.0]);
        apply_rig_linear(&mut img, &rig(Some(flat), Some(m))).unwrap();
        // Gain first: (1, 1, 1); then unmix: R = 1 - 0.5 = 0.5. Unmix first
        // would give R = 0.75.
        assert_eq!(img.pixel(0, 0), [0.5, 1.0, 1.0]);
    }

    #[test]
    fn size_mismatch_and_bad_parts_are_errors_and_leave_the_image() {
        let img = ramp(10, 8);
        let cases = [
            rig(Some(uniform_flat(10, 9, 2, 2, 1.0)), None),
            rig(Some(uniform_flat(11, 8, 2, 2, 1.0)), None),
            rig(Some(uniform_flat(10, 8, 0, 0, 1.0)), None),
            rig(Some(uniform_flat(10, 8, 2, 2, f32::NAN)), None),
            rig(Some(uniform_flat(10, 8, 2, 2, 0.0)), None),
            rig(
                Some(uniform_flat(10, 8, 2, 2, 1.5)),
                Some([[f32::INFINITY, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]),
            ),
        ];
        for r in cases {
            let mut out = img.clone();
            assert!(
                matches!(apply_rig_linear(&mut out, &r), Err(Error::Invalid(_))),
                "{r:?}"
            );
            assert_eq!(out, img);
        }
    }
}
