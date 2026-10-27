// SPDX-License-Identifier: MPL-2.0

//! The neutral-patch gamma fit of step 4 of `docs/MATHS.md`.

use negg_types::{Error, Result};

/// Smallest largest-`d_G` over the patches, in log-D, below which there is no
/// spread to fit. Step 4 of `docs/MATHS.md`; Provisional choice,
/// `TODO(measure)`.
pub const NEUTRAL_MIN_SPREAD: f32 = 0.1;

/// Result of [`fit_neutral`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NeutralFit {
    /// `gamma_c / gamma_G`; the G entry is 1.
    pub ratio: [f32; 3],
    /// Offsets in log-D (`o_c * gamma_G`); the G entry is 0.
    pub offset_log_d: [f32; 3],
    /// `gamma_G` times the RMS of the `2 n` e-space residuals over the `n`
    /// points used, log-D.
    pub residual_rms: f32,
    /// Points used: 2 when one grey patch plus the film base
    /// were fitted, else the number of grey patches `N`.
    pub patches: u32,
}

/// Fits the per-channel contrast ratios and offsets from neutral patches:
/// step 4 of `docs/MATHS.md`, "Neutral-patch fit".
///
/// `d_above_base` holds `d_n = max(D_n - Dmin, 0)` for `N >= 1` grey patches.
/// Only when `N == 1` is the film base `d = (0, 0, 0)` added as a second
/// point; with `N >= 2` the given patches alone are fitted, so two patches
/// give the exact solution and more give a least squares fit. For c in {R, B}
/// the ordinary least squares fit of `y = d_G / gamma_G` on `x = d_c` gives
/// slope `s_c = 1 / gamma_c` and intercept `o_c`; then
/// `ratio_c = 1 / (s_c gamma_G)` and `offset_c = o_c gamma_G`.
///
/// # Errors
///
/// No patches; `gamma_g` not finite and positive; a non-finite patch; the largest `d_G`
/// below [`NEUTRAL_MIN_SPREAD`]; a channel with no spread in `d_c`; a slope
/// that is not positive.
pub fn fit_neutral(d_above_base: &[[f32; 3]], gamma_g: f32) -> Result<NeutralFit> {
    if !gamma_g.is_finite() || gamma_g <= 0.0 {
        return Err(Error::Invalid(format!(
            "neutral fit: gamma_G must be positive, got {gamma_g}"
        )));
    }
    if d_above_base.is_empty() {
        return Err(Error::Invalid("neutral fit: no grey patches".into()));
    }
    if d_above_base.iter().flatten().any(|v| !v.is_finite()) {
        return Err(Error::Invalid(
            "neutral fit: non-finite patch density".into(),
        ));
    }
    let max_g = d_above_base.iter().map(|d| d[1]).fold(0.0f32, f32::max);
    if max_g < NEUTRAL_MIN_SPREAD {
        return Err(Error::Invalid(format!(
            "neutral fit: largest green density above base is {max_g}, \
             below the minimum spread {NEUTRAL_MIN_SPREAD} log-D"
        )));
    }

    let gg = f64::from(gamma_g);
    let mut used: Vec<[f32; 3]> = d_above_base.to_vec();
    if used.len() == 1 {
        used.push([0.0; 3]);
    }
    let n = used.len();
    let mut ratio = [1.0f32; 3];
    let mut offset = [0.0f32; 3];
    let mut sq = 0.0f64;
    for c in [0usize, 2] {
        let pts = used.iter().map(|d| (f64::from(d[c]), f64::from(d[1]) / gg));
        let nf = n as f64;
        let (sx, sy) = pts.clone().fold((0.0, 0.0), |a, p| (a.0 + p.0, a.1 + p.1));
        let (mx, my) = (sx / nf, sy / nf);
        let (mut sxx, mut sxy) = (0.0, 0.0);
        for (x, y) in pts.clone() {
            sxx += (x - mx) * (x - mx);
            sxy += (x - mx) * (y - my);
        }
        if sxx <= 1e-12 {
            return Err(Error::Invalid(
                "neutral fit: no spread in the red or blue patch densities".into(),
            ));
        }
        let slope = sxy / sxx;
        if slope <= 0.0 {
            return Err(Error::Invalid(format!(
                "neutral fit: slope {slope} for channel {c} is not positive"
            )));
        }
        let o = my - slope * mx;
        for (x, y) in pts {
            let e = y - (slope * x + o);
            sq += e * e;
        }
        ratio[c] = (1.0 / (slope * gg)) as f32;
        offset[c] = (o * gg) as f32;
    }
    let residual_rms = (gg * (sq / (2.0 * n as f64)).sqrt()) as f32;
    Ok(NeutralFit {
        ratio,
        offset_log_d: offset,
        residual_rms,
        patches: n as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GAMMA: [f32; 3] = [1.6, 1.8, 2.1];
    const OFFSET: [f32; 3] = [0.05, 0.0, -0.03];

    /// Density above base for neutral exposure `e`, from the model
    /// `d_c / gamma_c + o_c = d_G / gamma_G` with `d_G = gamma_G e`.
    fn patch(e: f32) -> [f32; 3] {
        let dg = GAMMA[1] * e;
        [GAMMA[0] * (e - OFFSET[0]), dg, GAMMA[2] * (e - OFFSET[2])]
    }

    #[test]
    fn two_patches_with_offsets_are_exact() {
        let fit = fit_neutral(&[patch(0.3), patch(0.7)], GAMMA[1]).unwrap();
        assert_eq!(fit.patches, 2);
        assert_eq!(fit.ratio[1], 1.0);
        assert_eq!(fit.offset_log_d[1], 0.0);
        for c in [0, 2] {
            assert!((fit.ratio[c] - GAMMA[c] / GAMMA[1]).abs() < 1e-5);
            // d_c / gamma_c = e - OFFSET_c, so y = e = s x + OFFSET_c and
            // offset = o_c * gamma_G = OFFSET_c * gamma_G.
            assert!((fit.offset_log_d[c] - OFFSET[c] * GAMMA[1]).abs() < 1e-5);
        }
        assert!(fit.residual_rms < 1e-5);
    }

    #[test]
    fn one_patch_plus_base_is_exact_for_zero_offsets() {
        let d = [GAMMA[0] * 0.5, GAMMA[1] * 0.5, GAMMA[2] * 0.5];
        let fit = fit_neutral(&[d], GAMMA[1]).unwrap();
        assert_eq!(fit.patches, 2);
        assert!((fit.ratio[0] - 1.6 / 1.8).abs() < 1e-5);
        assert!((fit.ratio[2] - 2.1 / 1.8).abs() < 1e-5);
        assert!(fit.offset_log_d[0].abs() < 1e-5);
        assert!(fit.offset_log_d[2].abs() < 1e-5);
        assert!(fit.residual_rms < 1e-5);
    }

    #[test]
    fn three_patches_give_the_ols_answer() {
        // Points used are the patches alone: x = [1, 2, 3], y = [1, 3, 3]
        // for both R and B, gamma_G = 1. mx = 2, my = 7/3, sxx = 2,
        // sxy = 2: slope 1, intercept 1/3, residuals -1/3, 2/3, -1/3.
        let d = [[1.0, 1.0, 1.0], [2.0, 3.0, 2.0], [3.0, 3.0, 3.0]];
        let fit = fit_neutral(&d, 1.0).unwrap();
        assert_eq!(fit.patches, 3);
        assert!((fit.ratio[0] - 1.0).abs() < 1e-6);
        assert!((fit.ratio[2] - 1.0).abs() < 1e-6);
        assert!((fit.offset_log_d[0] - 1.0 / 3.0).abs() < 1e-6);
        // sum sq = 6/9 per channel; rms = sqrt(2 * 6/9 / 6) = sqrt(2/9).
        assert!((fit.residual_rms - (2.0f32 / 9.0).sqrt()).abs() < 1e-6);
        // gamma_G = 2 with doubled green: y is unchanged, so the slope is 1
        // and ratio = 1 / (1 * 2) = 0.5; offset and residual double.
        let d2 = [[1.0, 2.0, 1.0], [2.0, 6.0, 2.0], [3.0, 6.0, 3.0]];
        let fit2 = fit_neutral(&d2, 2.0).unwrap();
        assert!((fit2.ratio[0] - 0.5).abs() < 1e-6);
        assert!((fit2.offset_log_d[0] - 2.0 / 3.0).abs() < 1e-6);
        assert!((fit2.residual_rms - 2.0 * (2.0f32 / 9.0).sqrt()).abs() < 1e-6);
    }

    #[test]
    fn no_patches_errors() {
        assert!(fit_neutral(&[], 1.8).is_err());
    }

    #[test]
    fn small_spread_errors() {
        assert!(fit_neutral(&[[0.05, 0.05, 0.05]], 1.8).is_err());
        assert!(fit_neutral(&[[0.05, 0.05, 0.05], [0.08, 0.09, 0.08]], 1.8).is_err());
    }

    #[test]
    fn non_positive_slope_errors() {
        let d = [[0.5, 0.5, 0.5], [0.0, 1.0, 1.0]];
        assert!(fit_neutral(&d, 1.8).is_err());
    }

    #[test]
    fn bad_gamma_errors() {
        let d = [[0.5, 0.5, 0.5]];
        assert!(fit_neutral(&d, 0.0).is_err());
        assert!(fit_neutral(&d, f32::NAN).is_err());
    }

    #[test]
    fn flat_red_errors() {
        assert!(fit_neutral(&[[0.0, 0.5, 0.5], [0.0, 0.9, 0.9]], 1.8).is_err());
    }
}
