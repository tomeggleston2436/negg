// SPDX-License-Identifier: MPL-2.0

//! Runtime forms of the phase 2 calibration profiles: the rig correction
//! (flat field, channel unmix, density model) and the film correction
//! (neutral-patch gamma ratios and offsets), plus [`PatchRect`].
//!
//! These are plain data. Building them from profile files is `negg-io` and
//! `negg-calib`; applying them is `negg-render`. Step 2 (rig), step 3
//! (density model) and step 4 (film correction) of `docs/MATHS.md` define
//! what each field means.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::image::Image;

/// Smallest per-cell flat field gain (ADR 0014).
pub const FLAT_GAIN_MIN: f32 = 0.25;
/// Largest per-cell flat field gain (ADR 0014).
pub const FLAT_GAIN_MAX: f32 = 4.0;
/// The flat is blurred with `sigma = (long_side_px / FLAT_SIGMA_DIVISOR)`
/// (ADR 0014).
pub const FLAT_SIGMA_DIVISOR: f32 = 64.0;

/// A flat field correction (step 2 of `docs/MATHS.md`): one gain per
/// channel per analysis grid cell, applied by bilinear interpolation.
#[derive(Clone, Debug, PartialEq)]
pub struct FlatField {
    /// Per-channel gain per grid cell, already clamped to
    /// [`FLAT_GAIN_MIN`, `FLAT_GAIN_MAX`].
    pub gain: Image,
    /// Width of the frame this correction applies to, in pixels.
    pub width: usize,
    /// Height of the frame this correction applies to, in pixels.
    pub height: usize,
}

/// The rig density model (step 3 of `docs/MATHS.md`):
/// `D_c = a_c + sum_k g[c][k] * log10(max(I_k, 1e-6))`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct DensityModel {
    /// Per-channel offset `a_c`.
    pub a: [f32; 3],
    /// Mixing matrix, row `c` (output channel), column `k` (input channel).
    pub g: [[f32; 3]; 3],
}

impl DensityModel {
    /// The model that reproduces the phase 1 density `-log10(I)`: zero
    /// offset and `g = -I`.
    pub const IDENTITY: DensityModel = DensityModel {
        a: [0.0; 3],
        g: [[-1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, -1.0]],
    };
}

/// A rig's correction as applied at render time (step 2 of
/// `docs/MATHS.md`). Each part is optional.
#[derive(Clone, Debug, PartialEq)]
pub struct RigCorrection {
    /// The profile name, for warnings and the sidecar.
    pub name: String,
    /// Flat field gains, if the profile has a flat field table.
    pub flat: Option<FlatField>,
    /// `M^-1`, the inverse of the crosstalk matrix, if the profile has one.
    pub unmix: Option<[[f32; 3]; 3]>,
    /// The density model, if the profile has a density fit.
    pub density: Option<DensityModel>,
}

/// A film stock's contrast correction as applied at render time (step 4 of
/// `docs/MATHS.md`).
#[derive(Clone, Debug, PartialEq)]
pub struct FilmCorrection {
    /// The profile name.
    pub name: String,
    /// The name of the rig the profile was measured with, if recorded.
    pub rig: Option<String>,
    /// `gamma_c / gamma_G` per channel; the green entry is 1.
    pub gamma_ratio: [f32; 3],
    /// Offset in log-density units per channel; the green entry is 0.
    pub offset_log_d: [f32; 3],
}

/// A rectangle in fractions of the frame, as in the manifest `grey_card`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PatchRect {
    /// Left edge, as a fraction of the frame width.
    pub x: f32,
    /// Top edge, as a fraction of the frame height.
    pub y: f32,
    /// Width, as a fraction of the frame width.
    pub width: f32,
    /// Height, as a fraction of the frame height.
    pub height: f32,
}

impl PatchRect {
    /// Checks the rectangle is finite, has positive size and lies inside
    /// the unit square.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Invalid`] naming the violated rule.
    pub fn validate(&self) -> Result<()> {
        let all = [self.x, self.y, self.width, self.height];
        if !all.iter().all(|v| v.is_finite()) {
            return Err(Error::Invalid(format!(
                "patch rect {self:?} must be finite"
            )));
        }
        if self.width <= 0.0 || self.height <= 0.0 {
            return Err(Error::Invalid(format!(
                "patch rect {self:?} must have positive width and height"
            )));
        }
        if self.x < 0.0 || self.y < 0.0 || self.x + self.width > 1.0 || self.y + self.height > 1.0 {
            return Err(Error::Invalid(format!(
                "patch rect {self:?} must lie inside [0, 1] on both axes"
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn density_model_identity_is_minus_identity() {
        let m = DensityModel::IDENTITY;
        assert_eq!(m.a, [0.0; 3]);
        for (c, row) in m.g.iter().enumerate() {
            for (k, v) in row.iter().enumerate() {
                assert_eq!(*v, if c == k { -1.0 } else { 0.0 });
            }
        }
    }

    #[test]
    fn density_model_and_rect_round_trip() {
        let m = DensityModel {
            a: [0.1, 0.2, 0.3],
            g: [[-1.0, 0.05, 0.0], [0.02, -0.9, 0.01], [0.0, 0.03, -1.1]],
        };
        let back: DensityModel = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(back, m);
        let r = PatchRect {
            x: 0.25,
            y: 0.25,
            width: 0.5,
            height: 0.5,
        };
        let back: PatchRect = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn patch_rect_validate() {
        let ok = PatchRect {
            x: 0.0,
            y: 0.5,
            width: 1.0,
            height: 0.5,
        };
        ok.validate().unwrap();
        let bad = [
            PatchRect { width: 0.0, ..ok },
            PatchRect { height: -0.1, ..ok },
            PatchRect { x: -0.01, ..ok },
            PatchRect { y: 0.6, ..ok },
            PatchRect { x: 0.1, ..ok },
            PatchRect { x: f32::NAN, ..ok },
            PatchRect {
                width: f32::INFINITY,
                ..ok
            },
        ];
        for r in bad {
            assert!(matches!(r.validate(), Err(Error::Invalid(_))), "{r:?}");
        }
    }

    #[test]
    fn gain_constants_match_the_contract() {
        assert_eq!(FLAT_GAIN_MIN, 0.25);
        assert_eq!(FLAT_GAIN_MAX, 4.0);
        assert_eq!(FLAT_SIGMA_DIVISOR, 64.0);
    }
}
