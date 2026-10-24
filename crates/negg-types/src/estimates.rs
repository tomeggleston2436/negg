// SPDX-License-Identifier: MPL-2.0

//! Per-frame estimates produced by the analysis pass: film base, Dmax,
//! layer gamma, the exposure anchor and the cast correction.
//!
//! These are the numbers steps 3, 4, 5 and 6 of `docs/MATHS.md` derive from
//! a frame (or, in phase 2, from a roll). They are written into the sidecar
//! beside the source file so that a render is reproducible and so that a
//! user can tell a border-derived Dmin from a percentile one. Every field
//! is always present: an `Estimates` value is complete or it does not
//! exist, so no field carries `#[serde(default)]`. `Default` exists for
//! tests and placeholders only.

use serde::{Deserialize, Serialize};

/// Where a Dmin (film base) estimate came from. Step 3 of `docs/MATHS.md`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DminSource {
    /// The median of the flat cells in the unexposed border of the frame.
    Border,
    /// The 0.5th percentile of density over the analysis grid.
    Percentile,
    /// A value fixed by the user.
    Fixed,
    /// Pooled across the frames of a roll (phase 2).
    Roll,
    /// Taken from a film profile.
    Profile,
    /// In roll mode, per channel the lower of the roll's pooled Dmin and
    /// the frame's own percentile estimate, when some channels took one
    /// and some the other (Ruling 105, ADR 0029). A frame whose every
    /// channel took its own estimate records `Percentile`, and one whose
    /// every channel took the pool records `Roll`.
    RollMin,
}

/// Where a per-channel gamma estimate came from. Step 4 of `docs/MATHS.md`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GammaSource {
    /// The no-profile fallback `Dmax_c - Dmin_c`.
    Range,
    /// A value fixed by the user.
    Fixed,
    /// The neutral-patch least-squares fit (phase 2).
    NeutralFit,
    /// Pooled across the frames of a roll (phase 2).
    Roll,
    /// Taken from a film profile.
    Profile,
}

/// Where the step 6 cast correction came from (ADR 0018, R-cast-source).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CastSource {
    /// Fitted on this frame's analysis grid.
    Fit,
    /// Fixed by the user in the settings.
    Fixed,
    /// Pooled across the frames of a roll.
    Roll,
    /// The identity: auto colour is off, fewer than 8 cells survived, or a
    /// film correction is in use.
    Identity,
}

/// The per-channel log-affine cast correction of step 6 of `docs/MATHS.md`,
/// fitted on the analysis grid as `e_R = slope_r * e_G + offset_r` and
/// `e_B = slope_b * e_G + offset_b`.
///
/// The identity correction has slopes of 1 and offsets of 0.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CastCorrection {
    /// Fitted slope of `e_R` against `e_G`, bounded to [0.5, 2] (ADR 0027).
    pub slope_r: f32,
    /// Fitted offset of `e_R` against `e_G`.
    pub offset_r: f32,
    /// Fitted slope of `e_B` against `e_G`, bounded to [0.5, 2] (ADR 0027).
    pub slope_b: f32,
    /// Fitted offset of `e_B` against `e_G`.
    pub offset_b: f32,
    /// Number of grid cells that survived selection and fed the fit. Zero
    /// when the correction is the identity because too few cells survived.
    pub cells_used: u32,
}

impl CastCorrection {
    /// The correction that changes nothing: slopes 1, offsets 0, no cells.
    pub const IDENTITY: CastCorrection = CastCorrection {
        slope_r: 1.0,
        offset_r: 0.0,
        slope_b: 1.0,
        offset_b: 0.0,
        cells_used: 0,
    };
}

impl Default for CastCorrection {
    fn default() -> Self {
        CastCorrection::IDENTITY
    }
}

/// Everything the analysis pass estimated for one frame.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Estimates {
    /// Step 3 of `docs/MATHS.md`: film base density per channel.
    pub dmin: [f32; 3],
    /// Which path produced `dmin`.
    pub dmin_source: DminSource,
    /// Step 3 of `docs/MATHS.md`: the 99.5th-percentile density per channel.
    pub dmax: [f32; 3],
    /// Step 4 of `docs/MATHS.md`: layer gamma per channel.
    pub gamma: [f32; 3],
    /// Which path produced `gamma`.
    pub gamma_source: GammaSource,
    /// Step 4 of `docs/MATHS.md`: the film correction's per-channel offset in
    /// normalised log-exposure units (`offset_c / gamma_G`), green zero. Zero
    /// when no film correction is in use (R-contrast).
    pub neutral_offset: [f32; 3],
    /// Step 5 of `docs/MATHS.md`: the exposure anchor, a single luma value of
    /// normalised log exposure `e` that `x0` places at print density 0.75.
    pub anchor: f32,
    /// Step 5 of `docs/MATHS.md`: the solved exposure offset, before the
    /// user's exposure setting is added.
    pub x0: f32,
    /// Step 6 of `docs/MATHS.md`: the fitted cast correction.
    pub cast: CastCorrection,
    /// Which path produced `cast`.
    pub cast_source: CastSource,
}

impl Default for Estimates {
    /// Zeros everywhere, `Percentile` and `Range` sources and the identity
    /// cast. A placeholder for tests, not a usable estimate: a zero gamma
    /// cannot be inverted through.
    fn default() -> Self {
        Estimates {
            dmin: [0.0; 3],
            dmin_source: DminSource::Percentile,
            dmax: [0.0; 3],
            gamma: [0.0; 3],
            gamma_source: GammaSource::Range,
            neutral_offset: [0.0; 3],
            anchor: 0.0,
            x0: 0.0,
            cast: CastCorrection::IDENTITY,
            cast_source: CastSource::Identity,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_serialise_in_snake_case() {
        assert_eq!(
            serde_json::to_string(&DminSource::Percentile).unwrap(),
            "\"percentile\""
        );
        assert_eq!(
            serde_json::to_string(&GammaSource::NeutralFit).unwrap(),
            "\"neutral_fit\""
        );
        let back: DminSource = serde_json::from_str("\"border\"").unwrap();
        assert_eq!(back, DminSource::Border);
    }

    #[test]
    fn default_is_zero_percentile_range_identity() {
        let e = Estimates::default();
        assert_eq!(e.dmin, [0.0; 3]);
        assert_eq!(e.dmin_source, DminSource::Percentile);
        assert_eq!(e.gamma_source, GammaSource::Range);
        assert_eq!(e.cast, CastCorrection::IDENTITY);
        assert_eq!(CastCorrection::default(), CastCorrection::IDENTITY);
        assert_eq!(CastCorrection::IDENTITY.slope_r, 1.0);
        assert_eq!(CastCorrection::IDENTITY.offset_b, 0.0);
    }

    #[test]
    fn estimates_round_trip_through_json() {
        let e = Estimates {
            dmin: [0.21, 0.55, 0.78],
            dmin_source: DminSource::Border,
            dmax: [1.9, 2.2, 2.4],
            gamma: [1.69, 1.65, 1.62],
            gamma_source: GammaSource::Range,
            neutral_offset: [0.01, 0.0, -0.02],
            anchor: -0.42,
            x0: 1.25,
            cast: CastCorrection {
                slope_r: 1.05,
                offset_r: -0.01,
                slope_b: 0.97,
                offset_b: 0.02,
                cells_used: 123,
            },
            cast_source: CastSource::Fit,
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"dmin_source\":\"border\""));
        let back: Estimates = serde_json::from_str(&json).unwrap();
        assert_eq!(back, e);
    }

    #[test]
    fn cast_source_serialises_in_snake_case() {
        for (v, t) in [
            (CastSource::Fit, "\"fit\""),
            (CastSource::Fixed, "\"fixed\""),
            (CastSource::Roll, "\"roll\""),
            (CastSource::Identity, "\"identity\""),
        ] {
            assert_eq!(serde_json::to_string(&v).unwrap(), t);
            assert_eq!(serde_json::from_str::<CastSource>(t).unwrap(), v);
        }
        let e = Estimates::default();
        assert_eq!(e.neutral_offset, [0.0; 3]);
        assert_eq!(e.cast_source, CastSource::Identity);
    }

    #[test]
    fn estimates_require_every_field() {
        let err = serde_json::from_str::<Estimates>("{}");
        assert!(err.is_err(), "an incomplete estimate must not load");
    }
}
