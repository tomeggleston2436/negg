// SPDX-License-Identifier: MPL-2.0

//! Step 5 of `docs/MATHS.md`: the print stage (the tone curve).
//!
//! Working in print density `P_c`, with `e_c` the normalised log exposure
//! from step 4 (so `log10(T_c) = e_c`). Paper exposure is the negative's
//! transmittance (ADR 0011): a bright scene gives a dense negative, the most
//! negative `e_c`, the least paper exposure and so the brightest print.
//!
//! ```text
//! x_c   = e_c * k + x_0 + exposure + cmy_c
//! v_c   = Dmin_p + softplus(a_hl * (x_c - Dmin_p)) / a_hl    # shoulder into paper white
//! P_c   = Dmax_p - softplus(a_sh * (Dmax_p - v_c)) / a_sh    # toe into paper black
//! out_c = 10^(-P_c)                                          # scene-linear reflectance
//! ```
//!
//! with optional black point compensation `(out_c - 10^-Dmax_p) / (1 - 10^-Dmax_p)`.
//!
//! The grade `k` follows ADR 0006 (`k = 290 / R` clamped to `[2, 10]`) and
//! the exposure anchor `x_0` follows ADR 0008 (solved analytically so that
//! the anchor prints at `P = 0.75`). [`PrintCoefficients`] folds everything
//! that does not depend on the pixel into per-channel constants once per
//! render, so [`print_pixel`] is two softplus and one pow per channel.

use negg_types::settings::PrintSettings;
use negg_types::{Error, Image, Result, Settings};

use crate::par::for_each_pixel_mut;

/// Step 5 of `docs/MATHS.md`: the print density the exposure anchor lands on
/// (ADR 0008).
pub const ANCHOR_PRINT_DENSITY: f32 = 0.75;

/// Lower clamp of the grade `k` (step 5 of `docs/MATHS.md`).
const K_MIN: f64 = 2.0;
/// Upper clamp of the grade `k` (step 5 of `docs/MATHS.md`).
const K_MAX: f64 = 10.0;
/// Numerator of `k = 290 / R` (ADR 0006: `2.9 * range * 100` with `range = 1`).
const K_NUMERATOR: f64 = 290.0;

/// `softplus(z) = ln(1 + e^z)` in f64, as `max(z, 0) + ln(1 + e^-|z|)` so
/// that it never overflows for large `z` and never cancels for small `z`.
#[inline]
fn softplus_f64(z: f64) -> f64 {
    z.max(0.0) + (-z.abs()).exp().ln_1p()
}

/// `softplus_inv(y) = ln(e^y - 1)` in f64, as `y + ln(1 - e^-y)` with
/// `1 - e^-y` computed as `-expm1(-y)` so that small `y` keeps its precision
/// and large `y` cannot overflow. Only defined for `y > 0`.
#[inline]
fn softplus_inv_f64(y: f64) -> f64 {
    y + (-(-y).exp_m1()).ln()
}

/// Step 5 of `docs/MATHS.md`: `softplus(z) = ln(1 + e^z)`.
///
/// Computed in f64 in the numerically stable form `max(z, 0) + ln(1 + e^-|z|)`,
/// so it is finite for every finite `z`.
#[inline]
#[must_use]
pub fn softplus(z: f32) -> f32 {
    softplus_f64(f64::from(z)) as f32
}

/// Step 5 of `docs/MATHS.md` (ADR 0008): the inverse softplus
/// `softplus_inv(y) = ln(e^y - 1)`, used to solve the exposure anchor.
///
/// Computed in f64. `y` must be positive; the result is `-inf` at 0 and NaN
/// below it, matching the mathematical function.
#[inline]
#[must_use]
pub fn softplus_inv(y: f32) -> f32 {
    softplus_inv_f64(f64::from(y)) as f32
}

/// `k` for a grade `R`, in f64, per ADR 0006.
#[inline]
fn grade_k_f64(r: f64) -> f64 {
    (K_NUMERATOR / r).clamp(K_MIN, K_MAX)
}

/// Step 5 of `docs/MATHS.md` (ADR 0006): the grade `k` for an ISO-R style
/// number `R`: `k = 2.9 * range / (R / 100)` with `range = 1` by
/// construction of step 4, so `k = 290 / R`, clamped to `[2, 10]`.
///
/// `R = 115` gives 2.5217, `R = 50` gives 5.8 and `R = 180` gives 2.0 (after
/// the clamp from 1.611).
#[inline]
#[must_use]
pub fn grade_k(r: f32) -> f32 {
    grade_k_f64(f64::from(r)) as f32
}

/// Step 5 of `docs/MATHS.md` (ADR 0008): solves `x_0` so that the exposure
/// anchor `anchor_e` (a luma value of `e`, normally in `[-1, 0]`) prints at
/// `P = 0.75` with the grade, paper densities, shoulder and toe in `print`.
///
/// The user's `print.exposure` and `print.cmy` are deliberately not
/// included: they are added on top by [`PrintCoefficients::new`].
///
/// The solution is analytic:
///
/// ```text
/// v        = Dmax_p - softplus_inv(a_sh * (Dmax_p - 0.75)) / a_sh
/// x_target = Dmin_p + softplus_inv(a_hl * (v - Dmin_p)) / a_hl
/// x_0      = x_target - anchor_e * k
/// ```
///
/// It only exists when `P = 0.75` is reachable, which needs roughly
/// `Dmin_p < 0.75 < Dmax_p`; outside that the result is not finite and
/// [`PrintCoefficients::new`] rejects it.
#[must_use]
pub fn solve_x0(anchor_e: f32, print: &PrintSettings) -> f32 {
    let dmin_p = f64::from(print.paper_dmin);
    let dmax_p = f64::from(print.paper_dmax);
    let a_hl = f64::from(print.shoulder);
    let a_sh = f64::from(print.toe);
    let k = grade_k_f64(f64::from(print.grade));
    let target = f64::from(ANCHOR_PRINT_DENSITY);
    let v = dmax_p - softplus_inv_f64(a_sh * (dmax_p - target)) / a_sh;
    let x_target = dmin_p + softplus_inv_f64(a_hl * (v - dmin_p)) / a_hl;
    (x_target - f64::from(anchor_e) * k) as f32
}

/// Step 5 of `docs/MATHS.md`: every per-channel coefficient of the print
/// stage, precomputed once per render so that [`print_pixel`] is two
/// softplus and one pow per channel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PrintCoefficients {
    /// Grade `k`, from [`grade_k`].
    k: f32,
    /// `x_0 + exposure + cmy_c` per channel.
    x0_total: [f32; 3],
    /// Paper white density `Dmin_p`.
    dmin_p: f32,
    /// Paper black density `Dmax_p`.
    dmax_p: f32,
    /// Shoulder steepness `a_hl`.
    a_hl: f32,
    /// Toe steepness `a_sh`.
    a_sh: f32,
    /// Whether black point compensation is applied.
    bpc: bool,
    /// `10^-Dmax_p`, the reflectance of paper black.
    floor: f32,
    /// `1 - 10^-Dmax_p`, the black point compensation denominator.
    bpc_denominator: f32,
}

impl PrintCoefficients {
    /// Step 5 of `docs/MATHS.md`: folds `print` and the solved `x0` (from
    /// [`solve_x0`]) into per-channel coefficients. `print.exposure` and
    /// `print.cmy` are added to `x0` here.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Invalid`] when `print` fails the same rules as
    /// [`Settings::validate`] (grade in 50 to 180, positive shoulder and
    /// toe, `paper_dmax > paper_dmin`, all finite) or when `x0` is not
    /// finite.
    pub fn new(print: &PrintSettings, x0: f32) -> Result<PrintCoefficients> {
        Settings {
            print: print.clone(),
            ..Settings::default()
        }
        .validate()?;
        if !x0.is_finite() {
            return Err(Error::Invalid(format!("x0 {x0} must be finite")));
        }
        let floor = 10f32.powf(-print.paper_dmax);
        let base = x0 + print.exposure;
        Ok(PrintCoefficients {
            k: grade_k(print.grade),
            x0_total: [
                base + print.cmy[0],
                base + print.cmy[1],
                base + print.cmy[2],
            ],
            dmin_p: print.paper_dmin,
            dmax_p: print.paper_dmax,
            a_hl: print.shoulder,
            a_sh: print.toe,
            bpc: print.black_point_compensation,
            floor,
            bpc_denominator: 1.0 - floor,
        })
    }

    /// Step 5 of `docs/MATHS.md`: the grade `k` in use.
    #[must_use]
    pub fn k(&self) -> f32 {
        self.k
    }

    /// Step 5 of `docs/MATHS.md`: `x_0 + exposure + cmy_c` per channel.
    #[must_use]
    pub fn x0_total(&self) -> [f32; 3] {
        self.x0_total
    }

    /// Step 5 of `docs/MATHS.md`: whether black point compensation is
    /// applied.
    #[must_use]
    pub fn black_point_compensation(&self) -> bool {
        self.bpc
    }
}

/// Step 5 of `docs/MATHS.md` for one pixel: maps normalised log exposure
/// `e_c` (step 4, after step 6) to scene-linear reflectance `out_c` in
/// `(0, 1]` (or `[0, 1]` with black point compensation).
///
/// Per channel: `x = e * k + x0_total_c` (ADR 0011), the softplus shoulder,
/// the softplus toe, `10^(-P)`, then `(out - 10^-Dmax_p) / (1 - 10^-Dmax_p)`
/// clipped at 0 when compensation is on. Monotone non-increasing in `e`: a
/// more negative `e` is a brighter scene and prints brighter.
#[inline]
#[must_use]
pub fn print_pixel(e: [f32; 3], c: &PrintCoefficients) -> [f32; 3] {
    let mut out = [0.0f32; 3];
    for ch in 0..3 {
        let x = c.x0_total[ch] + e[ch] * c.k;
        let v = c.dmin_p + softplus(c.a_hl * (x - c.dmin_p)) / c.a_hl;
        let p = c.dmax_p - softplus(c.a_sh * (c.dmax_p - v)) / c.a_sh;
        let mut o = 10f32.powf(-p);
        if c.bpc {
            o = ((o - c.floor) / c.bpc_denominator).max(0.0);
        }
        out[ch] = o;
    }
    out
}

/// Step 5 of `docs/MATHS.md`: applies [`print_pixel`] to every pixel of
/// `e`, in parallel over row bands, returning the positive.
#[must_use]
pub fn print(e: &Image, c: &PrintCoefficients) -> Image {
    let mut out = e.clone();
    for_each_pixel_mut(&mut out, |px| print_pixel(px, c));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn defaults() -> PrintSettings {
        PrintSettings::default()
    }

    /// The four equations of step 5 written out directly in f64, with no
    /// precomputation, for checking the kernel against.
    fn reference(e: f64, p: &PrintSettings, x0: f64, cmy: f64) -> f64 {
        let k = (290.0 / f64::from(p.grade)).clamp(2.0, 10.0);
        let dmin = f64::from(p.paper_dmin);
        let dmax = f64::from(p.paper_dmax);
        let a_hl = f64::from(p.shoulder);
        let a_sh = f64::from(p.toe);
        let x = e * k + x0 + f64::from(p.exposure) + cmy;
        let v = dmin + (1.0 + (a_hl * (x - dmin)).exp()).ln() / a_hl;
        let pd = dmax - (1.0 + (a_sh * (dmax - v)).exp()).ln() / a_sh;
        let out = 10f64.powf(-pd);
        if p.black_point_compensation {
            let floor = 10f64.powf(-dmax);
            ((out - floor) / (1.0 - floor)).max(0.0)
        } else {
            out
        }
    }

    #[test]
    fn grade_k_matches_adr_0006() {
        assert!((grade_k(115.0) - 2.5217).abs() < 1e-4);
        assert!((grade_k(50.0) - 5.8).abs() < 1e-6);
        assert!((grade_k(180.0) - 2.0).abs() < 1e-6);
        assert!((grade_k(145.0) - 2.0).abs() < 1e-6);
        assert!((grade_k(20.0) - 10.0).abs() < 1e-6);
    }

    #[test]
    fn softplus_is_stable_at_the_extremes() {
        assert_eq!(softplus(100.0), 100.0);
        assert_eq!(softplus(1000.0), 1000.0);
        assert_eq!(softplus(-1000.0), 0.0);
        assert!((softplus(0.0) - std::f32::consts::LN_2).abs() < 1e-6);
        assert!(softplus_inv(1e-6).is_finite());
        assert!((softplus_inv(100.0) - 100.0).abs() < 1e-5);
    }

    #[test]
    fn anchor_prints_at_0_75() {
        let p = defaults();
        for anchor in [-0.9f32, -0.5, -0.3, -0.1, 0.0] {
            let x0 = solve_x0(anchor, &p);
            let c = PrintCoefficients::new(&p, x0).unwrap();
            let out = print_pixel([anchor; 3], &c);
            let want = 10f32.powf(-ANCHOR_PRINT_DENSITY);
            for o in out {
                assert!(
                    ((o - want) / want).abs() < 1e-4,
                    "anchor {anchor}: out {o} want {want}"
                );
            }
        }
    }

    #[test]
    fn exposure_and_cmy_are_added_to_x0() {
        let mut p = defaults();
        p.exposure = 0.25;
        p.cmy = [0.1, -0.05, 0.0];
        let c = PrintCoefficients::new(&p, 1.5).unwrap();
        let t = c.x0_total();
        assert!((t[0] - 1.85).abs() < 1e-6);
        assert!((t[1] - 1.70).abs() < 1e-6);
        assert!((t[2] - 1.75).abs() < 1e-6);
        assert!((c.k() - grade_k(115.0)).abs() < 1e-7);
        assert!(!c.black_point_compensation());
        // Positive exposure means more print density, so a darker print.
        let darker = print_pixel([-0.5; 3], &c)[2];
        let base = print_pixel(
            [-0.5; 3],
            &PrintCoefficients::new(&defaults(), 1.5).unwrap(),
        )[2];
        assert!(darker < base);
    }

    #[test]
    fn new_rejects_invalid_settings() {
        let ok = defaults();
        assert!(PrintCoefficients::new(&ok, 0.0).is_ok());
        assert!(PrintCoefficients::new(&ok, f32::NAN).is_err());
        assert!(PrintCoefficients::new(&ok, f32::INFINITY).is_err());
        let mut p = defaults();
        p.grade = 49.0;
        assert!(PrintCoefficients::new(&p, 0.0).is_err());
        p.grade = 181.0;
        assert!(PrintCoefficients::new(&p, 0.0).is_err());
        let mut p = defaults();
        p.shoulder = 0.0;
        assert!(PrintCoefficients::new(&p, 0.0).is_err());
        let mut p = defaults();
        p.toe = -1.0;
        assert!(PrintCoefficients::new(&p, 0.0).is_err());
        let mut p = defaults();
        p.paper_dmax = p.paper_dmin;
        assert!(PrintCoefficients::new(&p, 0.0).is_err());
        let mut p = defaults();
        p.exposure = f32::NAN;
        assert!(PrintCoefficients::new(&p, 0.0).is_err());
    }

    #[test]
    fn black_point_compensation_reaches_zero_and_clips() {
        let mut p = defaults();
        p.black_point_compensation = true;
        let c = PrintCoefficients::new(&p, 0.0).unwrap();
        assert!(c.black_point_compensation());
        // Far into the toe (a thin negative, large positive e) the print
        // sits at paper black, which maps to 0.
        let dark = print_pixel([100.0; 3], &c);
        for o in dark {
            assert!((0.0..1e-6).contains(&o), "dark {o}");
        }
        // Paper white (a dense negative, very negative e) is at most 1.
        let light = print_pixel([-100.0; 3], &c);
        for o in light {
            assert!(o <= 1.0 && o > 0.5, "light {o}");
        }
    }

    #[test]
    fn print_image_matches_print_pixel() {
        let p = defaults();
        let c = PrintCoefficients::new(&p, solve_x0(-0.4, &p)).unwrap();
        let mut img = Image::new(5, 3);
        for (i, v) in img.data_mut().iter_mut().enumerate() {
            *v = -(i as f32) / 45.0;
        }
        let out = print(&img, &c);
        for y in 0..3 {
            for x in 0..5 {
                assert_eq!(out.pixel(x, y), print_pixel(img.pixel(x, y), &c));
            }
        }
        assert_eq!(out.width(), 5);
        assert_eq!(out.height(), 3);
    }

    proptest! {
        /// `softplus_inv` undoes `softplus` across the working range.
        #[test]
        fn softplus_round_trips(z in -10.0f32..10.0) {
            let back = softplus_inv(softplus(z));
            prop_assert!((back - z).abs() < 1e-4, "z {z} back {back}");
        }

        /// A denser negative (more negative `e`, a brighter scene) never
        /// prints darker, per channel (ADR 0011).
        #[test]
        fn monotone_non_increasing_in_e(
            e1 in -2.0f32..0.5, e2 in -2.0f32..0.5, grade in 50.0f32..180.0,
            exposure in -1.0f32..1.0, bpc in any::<bool>()
        ) {
            let mut p = defaults();
            p.grade = grade;
            p.exposure = exposure;
            p.black_point_compensation = bpc;
            let c = PrintCoefficients::new(&p, solve_x0(-0.5, &p)).unwrap();
            let (lo, hi) = if e1 <= e2 { (e1, e2) } else { (e2, e1) };
            let a = print_pixel([lo; 3], &c);
            let b = print_pixel([hi; 3], &c);
            for ch in 0..3 {
                prop_assert!(b[ch] <= a[ch], "lo {lo} -> {} hi {hi} -> {}", a[ch], b[ch]);
                prop_assert!(b[ch] >= 0.0 && a[ch] <= 1.0);
            }
        }

        /// A neutral `e` with equal coefficients per channel prints neutral.
        #[test]
        fn neutral_stays_neutral(e in -1.5f32..0.2, grade in 50.0f32..180.0, exposure in -1.0f32..1.0) {
            let mut p = defaults();
            p.grade = grade;
            p.exposure = exposure;
            let c = PrintCoefficients::new(&p, solve_x0(-0.5, &p)).unwrap();
            let out = print_pixel([e; 3], &c);
            prop_assert!((out[0] - out[1]).abs() < 1e-6);
            prop_assert!((out[1] - out[2]).abs() < 1e-6);
        }

        /// The precomputed kernel equals the four equations of step 5
        /// written out in f64 with no precomputation.
        #[test]
        fn kernel_matches_the_written_equations(
            e in -1.5f32..0.2, grade in 50.0f32..180.0, exposure in -1.0f32..1.0,
            cmy in -0.3f32..0.3, bpc in any::<bool>(), anchor in -1.0f32..0.0
        ) {
            let mut p = defaults();
            p.grade = grade;
            p.exposure = exposure;
            p.cmy = [cmy, 0.0, -cmy];
            p.black_point_compensation = bpc;
            let x0 = solve_x0(anchor, &p);
            let c = PrintCoefficients::new(&p, x0).unwrap();
            let out = print_pixel([e; 3], &c);
            for (ch, cm) in p.cmy.iter().enumerate() {
                let want = reference(f64::from(e), &p, f64::from(x0), f64::from(*cm));
                prop_assert!((f64::from(out[ch]) - want).abs() < 1e-5, "ch {ch} out {} want {want}", out[ch]);
            }
        }

        /// The anchor solution holds for every grade and anchor in range.
        #[test]
        fn anchor_holds_across_settings(
            anchor in -1.0f32..0.0, grade in 50.0f32..180.0,
            shoulder in 1.0f32..6.0, toe in 2.0f32..10.0
        ) {
            let mut p = defaults();
            p.grade = grade;
            p.shoulder = shoulder;
            p.toe = toe;
            let c = PrintCoefficients::new(&p, solve_x0(anchor, &p)).unwrap();
            let out = print_pixel([anchor; 3], &c)[1];
            let want = 10f32.powf(-ANCHOR_PRINT_DENSITY);
            prop_assert!(((out - want) / want).abs() < 1e-4, "out {out} want {want}");
        }
    }
}
