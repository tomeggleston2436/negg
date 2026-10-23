// SPDX-License-Identifier: MPL-2.0

//! Step 4 of `docs/MATHS.md`: inversion of density to relative log exposure.
//!
//! Density above the film base, clamped at 0, divided by the per-channel
//! layer gamma and negated:
//!
//! ```text
//! d_c = max(D_c - Dmin_c, 0)
//! e_c = -d_c / gamma_c
//! ```
//!
//! With the no-profile fallback `gamma_c = Dmax_c - Dmin_c` each channel of
//! `e` spans `[-1, 0]`; a denser negative than the estimate goes below -1.
//! The linear positive `T_c = 10^(e_c)` is never materialised: step 5 works
//! directly on `e_c` (its `log10(T_c)` is just `e_c`).

use negg_types::{Error, Image, Result};

use crate::par::for_each_pixel_mut;

/// Checks the step 4 preconditions: every `gamma_c` is finite and positive
/// and every `dmin_c` is finite.
fn validate(dmin: [f32; 3], gamma: [f32; 3]) -> Result<()> {
    for (c, g) in gamma.iter().enumerate() {
        if !(g.is_finite() && *g > 0.0) {
            return Err(Error::Invalid(format!(
                "gamma[{c}] {g} must be finite and positive"
            )));
        }
    }
    for (c, d) in dmin.iter().enumerate() {
        if !d.is_finite() {
            return Err(Error::Invalid(format!("dmin[{c}] {d} must be finite")));
        }
    }
    Ok(())
}

/// Step 4 of `docs/MATHS.md` for one pixel: `e_c = -max(D_c - dmin_c, 0) / gamma_c`.
///
/// This is the kernel shared by [`invert`], [`invert_in_place`] and
/// [`crate::render_full`], so the fused render is bitwise identical to the
/// composition. The caller is responsible for `gamma_c > 0`.
#[inline]
#[must_use]
pub fn invert_pixel(density: [f32; 3], dmin: [f32; 3], gamma: [f32; 3]) -> [f32; 3] {
    let mut e = [0.0f32; 3];
    for c in 0..3 {
        e[c] = -((density[c] - dmin[c]).max(0.0)) / gamma[c];
    }
    e
}

/// Step 4 of `docs/MATHS.md`: inverts a density image to normalised log
/// exposure, returning a new image. See [`invert_in_place`].
///
/// # Errors
///
/// Returns [`Error::Invalid`] when any `gamma_c` is not finite and positive
/// or any `dmin_c` is not finite.
pub fn invert(density: &Image, dmin: [f32; 3], gamma: [f32; 3]) -> Result<Image> {
    let mut e = density.clone();
    invert_in_place(&mut e, dmin, gamma)?;
    Ok(e)
}

/// Step 4 of `docs/MATHS.md`: inverts a density image in place, so that on
/// return `density` holds `e_c = -max(D_c - dmin_c, 0) / gamma_c` in
/// `(-inf, 0]`. Runs in parallel over row bands.
///
/// # Errors
///
/// Returns [`Error::Invalid`] when any `gamma_c` is not finite and positive
/// or any `dmin_c` is not finite. The image is untouched on error.
pub fn invert_in_place(density: &mut Image, dmin: [f32; 3], gamma: [f32; 3]) -> Result<()> {
    validate(dmin, gamma)?;
    for_each_pixel_mut(density, |d| invert_pixel(d, dmin, gamma));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn full_range_maps_to_zero_and_minus_one() {
        let dmin = [0.2, 0.5, 0.7];
        let dmax = [1.9, 2.2, 2.4];
        let gamma = [dmax[0] - dmin[0], dmax[1] - dmin[1], dmax[2] - dmin[2]];
        let mut img = Image::new(2, 1);
        img.set_pixel(0, 0, dmin);
        img.set_pixel(1, 0, dmax);
        let e = invert(&img, dmin, gamma).unwrap();
        assert_eq!(e.pixel(0, 0), [0.0, 0.0, 0.0]);
        for c in 0..3 {
            assert!((e.pixel(1, 0)[c] + 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn below_base_clamps_to_zero() {
        let e = invert_pixel([0.1, 0.1, 0.1], [0.2, 0.2, 0.2], [1.0; 3]);
        assert_eq!(e, [0.0; 3]);
    }

    #[test]
    fn rejects_bad_gamma_and_dmin() {
        let img = Image::new(1, 1);
        assert!(invert(&img, [0.0; 3], [1.0, 0.0, 1.0]).is_err());
        assert!(invert(&img, [0.0; 3], [1.0, -1.0, 1.0]).is_err());
        assert!(invert(&img, [0.0; 3], [1.0, f32::NAN, 1.0]).is_err());
        assert!(invert(&img, [f32::INFINITY, 0.0, 0.0], [1.0; 3]).is_err());
        let mut keep = Image::new(1, 1);
        keep.set_pixel(0, 0, [0.5; 3]);
        let before = keep.clone();
        assert!(invert_in_place(&mut keep, [0.0; 3], [0.0; 3]).is_err());
        assert_eq!(keep, before);
    }

    proptest! {
        /// A neutral density (equal channels) with equal dmin and gamma
        /// per channel inverts to a neutral `e`.
        #[test]
        fn neutral_stays_neutral(d in 0.0f32..3.0, dmin in 0.0f32..1.0, gamma in 0.1f32..3.0) {
            let e = invert_pixel([d; 3], [dmin; 3], [gamma; 3]);
            prop_assert_eq!(e[0], e[1]);
            prop_assert_eq!(e[1], e[2]);
        }

        /// `e` is non-increasing in density and never positive.
        #[test]
        fn monotone_non_increasing_in_density(
            d1 in 0.0f32..3.0, d2 in 0.0f32..3.0, dmin in 0.0f32..1.0, gamma in 0.1f32..3.0
        ) {
            let (lo, hi) = if d1 <= d2 { (d1, d2) } else { (d2, d1) };
            let e_lo = invert_pixel([lo; 3], [dmin; 3], [gamma; 3])[0];
            let e_hi = invert_pixel([hi; 3], [dmin; 3], [gamma; 3])[0];
            prop_assert!(e_hi <= e_lo);
            prop_assert!(e_lo <= 0.0);
        }
    }
}
