// SPDX-License-Identifier: MPL-2.0

//! Steps 3 to 7 of `docs/MATHS.md` fused into one pass per pixel.
//!
//! [`render_full`] runs the density transform (step 3, with
//! `negg_film::density_value`, or [`crate::density_model_pixel`] with a rig
//! density model), inversion (step 4, then the film correction's neutral
//! offset), the cast correction (step 6),
//! the print stage (step 5) and the optional camera matrix (step 7) on each
//! pixel in turn, over row bands in parallel, without materialising the
//! intermediate images, not even the full-size density image. It calls
//! the same per-pixel kernels as the separate functions
//! (`negg_film::density_value`, [`crate::invert_pixel`], [`crate::apply_cast_pixel`],
//! [`crate::print_pixel`], [`crate::apply_matrix_pixel`]), so the result is
//! identical to their composition.

use negg_film::density_value;
use negg_types::{DensityModel, Error, Estimates, Image, Result};

use crate::cast::apply_cast_pixel;
use crate::invert::{invert_in_place, invert_pixel};
use crate::matrix::apply_matrix_pixel;
use crate::par::for_each_pixel_mut;
use crate::print::{PrintCoefficients, print_pixel};
use crate::rig::density_model_pixel;

/// Steps 3 to 7 of `docs/MATHS.md` in one pass: for every pixel of
/// `linear` (linear camera RGB, or linear Rec.2020 for an ICC input), take
/// the density `D = -log10(max(I, 1e-6))` (step 3), or the rig density
/// model through [`density_model_pixel`] when `density` is given
/// (R-density-use, ADR 0015), invert with `est.dmin` and `est.gamma` and
/// subtract `est.neutral_offset` (step 4, R-contrast:
/// `e_c = -d_c / gamma_c - offset_c / gamma_G`), apply `est.cast` (step 6),
/// print with `print` (step 5), then apply `matrix` with a clip at 0 when
/// one is given (step 7, unchanged by a rig, R-step7). Runs in parallel
/// over row bands and allocates only the output image.
///
/// `print` should have been built from `est.x0` (see
/// [`PrintCoefficients::new`]); it is passed separately so the caller can
/// re-render with new print settings without re-estimating.
///
/// # Errors
///
/// Returns [`negg_types::Error::Invalid`] when `est.gamma` has a channel
/// that is not finite and positive or `est.dmin` is not finite, exactly as
/// [`crate::invert`] does, or when `est.neutral_offset` is not finite.
pub fn render_full(
    linear: &Image,
    est: &Estimates,
    print: &PrintCoefficients,
    density: Option<&DensityModel>,
    matrix: Option<[[f32; 3]; 3]>,
) -> Result<Image> {
    // Validate step 4's preconditions up front on an empty image so the
    // fused kernel below cannot divide by a zero gamma.
    invert_in_place(&mut Image::new(0, 0), est.dmin, est.gamma)?;
    if !est.neutral_offset.iter().all(|o| o.is_finite()) {
        return Err(Error::Invalid(format!(
            "neutral_offset {:?} must be finite",
            est.neutral_offset
        )));
    }
    let dmin = est.dmin;
    let gamma = est.gamma;
    let offset = est.neutral_offset;
    let cast = est.cast;
    let mut out = linear.clone();
    for_each_pixel_mut(&mut out, |i| {
        let d = match density {
            Some(m) => density_model_pixel(i, m),
            None => [
                density_value(i[0]),
                density_value(i[1]),
                density_value(i[2]),
            ],
        };
        let e = invert_pixel(d, dmin, gamma);
        let e = [e[0] - offset[0], e[1] - offset[1], e[2] - offset[2]];
        let e = apply_cast_pixel(e, &cast);
        let positive = print_pixel(e, print);
        match matrix {
            Some(m) => apply_matrix_pixel(positive, m),
            None => positive,
        }
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{apply_cast, apply_matrix, density_grid, invert, print, solve_x0};
    use negg_types::settings::PrintSettings;
    use negg_types::{CastCorrection, DminSource, GammaSource};

    /// A small xorshift generator so the random image has a fixed seed and
    /// no dependency beyond std.
    struct XorShift(u32);

    impl XorShift {
        fn next_unit(&mut self) -> f32 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            self.0 = x;
            (x >> 8) as f32 / (1u32 << 24) as f32
        }
    }

    /// Linear transmittance whose density is uniform in `[0.1, 2.7]`.
    fn random_linear(w: usize, h: usize, seed: u32) -> Image {
        let mut rng = XorShift(seed);
        let mut img = Image::new(w, h);
        for v in img.data_mut() {
            *v = 10f32.powf(-(0.1 + 2.6 * rng.next_unit()));
        }
        img
    }

    fn estimates() -> Estimates {
        Estimates {
            dmin: [0.21, 0.55, 0.78],
            dmin_source: DminSource::Border,
            dmax: [1.9, 2.2, 2.4],
            gamma: [1.69, 1.65, 1.62],
            gamma_source: GammaSource::Range,
            neutral_offset: [0.0; 3],
            anchor: -0.42,
            x0: 0.0,
            cast: CastCorrection {
                slope_r: 1.05,
                offset_r: -0.01,
                slope_b: 0.97,
                offset_b: 0.02,
                cells_used: 40,
            },
            cast_source: negg_types::CastSource::Fit,
        }
    }

    fn composition(
        linear: &Image,
        est: &Estimates,
        c: &PrintCoefficients,
        model: Option<&DensityModel>,
        matrix: Option<[[f32; 3]; 3]>,
    ) -> Image {
        let mut e = invert(&density_grid(linear, model), est.dmin, est.gamma).unwrap();
        for px in e.data_mut().as_chunks_mut::<3>().0 {
            for (v, o) in px.iter_mut().zip(est.neutral_offset) {
                *v -= o;
            }
        }
        apply_cast(&mut e, &est.cast);
        let mut out = print(&e, c);
        if let Some(m) = matrix {
            apply_matrix(&mut out, m);
        }
        out
    }

    fn assert_close(a: &Image, b: &Image) {
        assert_eq!(a.width(), b.width());
        assert_eq!(a.height(), b.height());
        for (i, (x, y)) in a.data().iter().zip(b.data()).enumerate() {
            assert!((x - y).abs() <= 1e-5, "float {i}: fused {x} separate {y}");
        }
    }

    #[test]
    fn fused_render_equals_the_composition() {
        let linear = random_linear(97, 61, 0x9E37_79B9);
        let mut est = estimates();
        let mut settings = PrintSettings {
            exposure: 0.1,
            cmy: [0.02, 0.0, -0.03],
            ..PrintSettings::default()
        };
        est.x0 = solve_x0(est.anchor, &settings);
        let c = PrintCoefficients::new(&settings, est.x0).unwrap();
        let m = [[0.9, 0.1, 0.0], [0.05, 0.9, 0.05], [0.0, 0.2, 0.8]];

        let fused = render_full(&linear, &est, &c, None, Some(m)).unwrap();
        assert_close(&fused, &composition(&linear, &est, &c, None, Some(m)));

        let fused = render_full(&linear, &est, &c, None, None).unwrap();
        assert_close(&fused, &composition(&linear, &est, &c, None, None));

        settings.black_point_compensation = true;
        let c = PrintCoefficients::new(&settings, est.x0).unwrap();
        let fused = render_full(&linear, &est, &c, None, Some(m)).unwrap();
        assert_close(&fused, &composition(&linear, &est, &c, None, Some(m)));
    }

    #[test]
    fn fused_render_with_a_density_model_and_offsets_equals_the_composition() {
        let linear = random_linear(83, 47, 0x1234_5678);
        let model = DensityModel {
            a: [0.05, -0.02, 0.1],
            g: [[-1.05, 0.04, 0.01], [0.03, -0.95, 0.02], [0.0, 0.06, -1.1]],
        };
        let mut est = estimates();
        est.neutral_offset = [0.031, 0.0, -0.022];
        let settings = PrintSettings::default();
        est.x0 = solve_x0(est.anchor, &settings);
        let c = PrintCoefficients::new(&settings, est.x0).unwrap();
        let m = [[0.9, 0.1, 0.0], [0.05, 0.9, 0.05], [0.0, 0.2, 0.8]];
        for matrix in [None, Some(m)] {
            let fused = render_full(&linear, &est, &c, Some(&model), matrix).unwrap();
            assert_close(
                &fused,
                &composition(&linear, &est, &c, Some(&model), matrix),
            );
        }
        // The identity model renders exactly as no model.
        let a = render_full(&linear, &est, &c, Some(&DensityModel::IDENTITY), None).unwrap();
        let b = render_full(&linear, &est, &c, None, None).unwrap();
        assert_eq!(a, b);
        // The offsets move R and B and leave G alone.
        let zero = Estimates {
            neutral_offset: [0.0; 3],
            ..est
        };
        let z = render_full(&linear, &zero, &c, None, None).unwrap();
        for (p, q) in b
            .data()
            .as_chunks::<3>()
            .0
            .iter()
            .zip(z.data().as_chunks::<3>().0)
        {
            assert_eq!(p[1], q[1]);
        }
        assert_ne!(b, z);
    }

    #[test]
    fn rejects_non_finite_neutral_offset() {
        let linear = Image::new(2, 2);
        let mut est = estimates();
        est.neutral_offset = [f32::NAN, 0.0, 0.0];
        let c = PrintCoefficients::new(&PrintSettings::default(), 0.0).unwrap();
        assert!(matches!(
            render_full(&linear, &est, &c, None, None),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn output_is_in_range_without_a_matrix() {
        let linear = random_linear(40, 30, 7);
        let est = estimates();
        let c = PrintCoefficients::new(&PrintSettings::default(), 1.0).unwrap();
        let out = render_full(&linear, &est, &c, None, None).unwrap();
        for v in out.data() {
            assert!(*v > 0.0 && *v <= 1.0, "{v}");
        }
    }

    #[test]
    fn neutral_ramp_stays_neutral_through_the_whole_pipeline() {
        let mut linear = Image::new(64, 1);
        for (x, px) in linear
            .data_mut()
            .as_chunks_mut::<3>()
            .0
            .iter_mut()
            .enumerate()
        {
            let d = 0.2 + 2.0 * x as f32 / 63.0;
            px.copy_from_slice(&[10f32.powf(-d); 3]);
        }
        let est = Estimates {
            dmin: [0.2; 3],
            dmax: [2.2; 3],
            gamma: [2.0; 3],
            cast: CastCorrection::IDENTITY,
            ..Estimates::default()
        };
        let settings = PrintSettings::default();
        let c = PrintCoefficients::new(&settings, solve_x0(-0.5, &settings)).unwrap();
        let out = render_full(&linear, &est, &c, None, None).unwrap();
        let mut previous = -1.0f32;
        for px in out.data().as_chunks::<3>().0 {
            assert!(
                (px[0] - px[1]).abs() < 1e-6 && (px[1] - px[2]).abs() < 1e-6,
                "{px:?}"
            );
            // Denser negative prints brighter (ADR 0011): the ramp is
            // non-decreasing.
            assert!(px[0] >= previous - 1e-7, "{} after {previous}", px[0]);
            previous = px[0];
        }
    }

    #[test]
    fn rejects_bad_gamma() {
        let linear = Image::new(2, 2);
        let est = Estimates::default();
        let c = PrintCoefficients::new(&PrintSettings::default(), 0.0).unwrap();
        assert!(render_full(&linear, &est, &c, None, None).is_err());
    }
}
