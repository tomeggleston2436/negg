// SPDX-License-Identifier: MPL-2.0

//! Conversion settings for one frame ([`Settings`]) and for a whole roll
//! ([`RollSettings`]).
//!
//! Settings are plain serde structs. Every struct carries
//! `#[serde(default)]`, so a sidecar that omits a field (or an empty `{}`)
//! loads with the plan's defaults, and unknown fields are ignored so that
//! sidecars written by a newer build still load. Settings are immutable once
//! written and are identified by their [`ContentHash`]; the settings history
//! that backs undo is a list of such hashes.
//!
//! Field documentation names the step of `docs/MATHS.md` that each parameter
//! belongs to.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::hash::{ContentHash, hash_value};

/// Version of the settings schema written into sidecars.
pub const SETTINGS_SCHEMA_VERSION: u32 = 1;

/// A per-channel value that is either estimated from the frame (or roll) or
/// fixed by the user.
///
/// Serialises as `{"mode": "auto"}` or
/// `{"mode": "fixed", "value": [r, g, b]}`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ChannelEstimate {
    /// Estimate the value automatically.
    #[default]
    Auto,
    /// Use the given per-channel value, locked or overridden by the user.
    Fixed {
        /// The value for the red, green and blue channels.
        value: [f32; 3],
    },
}

impl ChannelEstimate {
    /// The fixed value, if any.
    fn fixed(&self) -> Option<[f32; 3]> {
        match self {
            ChannelEstimate::Auto => None,
            ChannelEstimate::Fixed { value } => Some(*value),
        }
    }
}

/// Film model parameters: steps 3 and 4 of `docs/MATHS.md`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FilmSettings {
    /// Step 3 of `docs/MATHS.md`: Dmin, the film base density per channel.
    /// Estimated automatically unless locked or overridden.
    pub film_base: ChannelEstimate,
    /// Step 3 of `docs/MATHS.md`: Dmax per channel. Estimated automatically
    /// unless locked or overridden.
    pub dmax: ChannelEstimate,
    /// Step 4 of `docs/MATHS.md`: per-channel gamma. When `Auto` it is
    /// `Dmax_c - Dmin_c` for each channel.
    pub gamma: ChannelEstimate,
}

impl Default for FilmSettings {
    fn default() -> Self {
        FilmSettings {
            film_base: ChannelEstimate::Auto,
            dmax: ChannelEstimate::Auto,
            gamma: ChannelEstimate::Auto,
        }
    }
}

/// Print stage parameters: step 5 of `docs/MATHS.md`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrintSettings {
    /// Step 5 of `docs/MATHS.md`: grade `k`, expressed as an ISO-R style
    /// number. Default 115, valid range 50 to 180 inclusive.
    pub grade: f32,
    /// Step 5 of `docs/MATHS.md`: the user's exposure offset, in log-density
    /// units, added to the automatically anchored `x_0`. Default 0.
    pub exposure: f32,
    /// Step 5 of `docs/MATHS.md`: `cmy_c`, the three colour timing offsets in
    /// log space. Default `[0, 0, 0]`.
    pub cmy: [f32; 3],
    /// Step 5 of `docs/MATHS.md`: `Dmin_p`, the paper's minimum density.
    /// Default 0.06.
    pub paper_dmin: f32,
    /// Step 5 of `docs/MATHS.md`: `Dmax_p`, the paper's maximum density.
    /// Default 2.3.
    pub paper_dmax: f32,
    /// Step 5 of `docs/MATHS.md`: `a_hl`, the highlight shoulder steepness.
    /// Default 3.0.
    pub shoulder: f32,
    /// Step 5 of `docs/MATHS.md`: `a_sh`, the shadow toe steepness.
    /// Default 6.0.
    pub toe: f32,
    /// Step 5 of `docs/MATHS.md`: optional black point compensation.
    /// Default off.
    pub black_point_compensation: bool,
}

impl Default for PrintSettings {
    fn default() -> Self {
        PrintSettings {
            grade: 115.0,
            exposure: 0.0,
            cmy: [0.0; 3],
            paper_dmin: 0.06,
            paper_dmax: 2.3,
            shoulder: 3.0,
            toe: 6.0,
            black_point_compensation: false,
        }
    }
}

/// Colour handling: steps 6 and 7 of `docs/MATHS.md`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColourSettings {
    /// Step 6 of `docs/MATHS.md`: automatic colour cast removal. Default on.
    pub auto_colour: bool,
    /// Step 7 of `docs/MATHS.md`: the output colour space. Default sRGB.
    pub output_space: OutputSpace,
}

impl Default for ColourSettings {
    fn default() -> Self {
        ColourSettings {
            auto_colour: true,
            output_space: OutputSpace::Srgb,
        }
    }
}

/// Output colour spaces offered by step 7 of `docs/MATHS.md`.
///
/// Serialises in snake case: `"srgb"`, `"adobe_rgb"`, `"display_p3"`,
/// `"pro_photo"`, `"rec2020"`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputSpace {
    /// sRGB (IEC 61966-2-1).
    #[default]
    Srgb,
    /// Adobe RGB (1998).
    AdobeRgb,
    /// Display P3.
    DisplayP3,
    /// ProPhoto RGB (ROMM).
    ProPhoto,
    /// ITU-R BT.2020.
    Rec2020,
}

/// Every parameter needed to convert one frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Schema version of this struct, written into every sidecar (step 8 of
    /// `docs/MATHS.md`). Always [`SETTINGS_SCHEMA_VERSION`] for values
    /// produced by this build.
    pub schema_version: u32,
    /// Film model parameters (steps 3 and 4).
    pub film: FilmSettings,
    /// Print stage parameters (step 5).
    pub print: PrintSettings,
    /// Colour handling (steps 6 and 7).
    pub colour: ColourSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            schema_version: SETTINGS_SCHEMA_VERSION,
            film: FilmSettings::default(),
            print: PrintSettings::default(),
            colour: ColourSettings::default(),
        }
    }
}

/// Validation rule shared by `Settings::validate` helpers.
fn require(ok: bool, reason: impl FnOnce() -> String) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid(reason()))
    }
}

/// Every `f32` field of [`PrintSettings`] paired with its name, for the
/// finiteness rule in [`Settings::validate`].
fn print_floats(p: &PrintSettings) -> [(&'static str, f32); 9] {
    [
        ("grade", p.grade),
        ("exposure", p.exposure),
        ("cmy[0]", p.cmy[0]),
        ("cmy[1]", p.cmy[1]),
        ("cmy[2]", p.cmy[2]),
        ("paper_dmin", p.paper_dmin),
        ("paper_dmax", p.paper_dmax),
        ("shoulder", p.shoulder),
        ("toe", p.toe),
    ]
}

fn require_finite_channels(name: &str, estimate: ChannelEstimate) -> Result<()> {
    match estimate.fixed() {
        Some(value) => require(value.iter().all(|v| v.is_finite()), || {
            format!("{name} fixed value {value:?} must be finite")
        }),
        None => Ok(()),
    }
}

impl Settings {
    /// The content hash identifying this exact settings value.
    pub fn content_hash(&self) -> ContentHash {
        hash_value(self)
    }

    /// Checks the settings against the model's valid ranges.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Invalid`] when any `f32` field of [`PrintSettings`]
    /// is non-finite (the error names the field), the grade is outside 50 to
    /// 180, `paper_dmax` is not greater than `paper_dmin`, the shoulder or
    /// toe is not positive, or any fixed channel value is non-finite.
    pub fn validate(&self) -> Result<()> {
        let p = &self.print;
        for (name, value) in print_floats(p) {
            require(value.is_finite(), || {
                format!("{name} {value} must be finite")
            })?;
        }
        // Each positive comparison below evaluates false for NaN, so NaN
        // fails every rule.
        require((50.0..=180.0).contains(&p.grade), || {
            format!("grade {} must be between 50 and 180", p.grade)
        })?;
        require(p.paper_dmax > p.paper_dmin, || {
            format!(
                "paper_dmax {} must be greater than paper_dmin {}",
                p.paper_dmax, p.paper_dmin
            )
        })?;
        require(p.shoulder > 0.0, || {
            format!("shoulder {} must be positive", p.shoulder)
        })?;
        require(p.toe > 0.0, || format!("toe {} must be positive", p.toe))?;
        require_finite_channels("film_base", self.film.film_base)?;
        require_finite_channels("dmax", self.film.dmax)?;
        require_finite_channels("gamma", self.film.gamma)?;
        Ok(())
    }
}

/// Settings shared by every frame of a roll (phase 2 of the plan).
///
/// `shared` holds the defaults that each frame inherits; the `pool_*` flags
/// say which estimates the roll estimator pools across frames rather than
/// fitting per frame. A frame may still lock or override any value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RollSettings {
    /// Schema version, as for [`Settings::schema_version`].
    pub schema_version: u32,
    /// Defaults inherited by every frame in the roll.
    pub shared: Settings,
    /// Pool the Dmin (film base) estimate across frames. Default on.
    pub pool_film_base: bool,
    /// Pool the per-channel gamma estimate across frames. Default on.
    pub pool_gamma: bool,
    /// Pool the colour cast-removal fit across frames. Default on.
    pub pool_colour: bool,
}

impl Default for RollSettings {
    fn default() -> Self {
        RollSettings {
            schema_version: SETTINGS_SCHEMA_VERSION,
            shared: Settings::default(),
            pool_film_base: true,
            pool_gamma: true,
            pool_colour: true,
        }
    }
}

impl RollSettings {
    /// The content hash identifying this exact roll settings value.
    pub fn content_hash(&self) -> ContentHash {
        hash_value(self)
    }

    /// Validates the shared settings; see [`Settings::validate`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::Invalid`] when `shared` fails validation.
    pub fn validate(&self) -> Result<()> {
        self.shared.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_round_trips_through_json() {
        let s = Settings::default();
        let json = serde_json::to_string_pretty(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);

        let r = RollSettings::default();
        let json = serde_json::to_string(&r).unwrap();
        let back: RollSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn empty_object_is_default() {
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(s, Settings::default());
        let r: RollSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(r, RollSettings::default());
        let partial: Settings = serde_json::from_str(r#"{"print": {"grade": 90}}"#).unwrap();
        assert_eq!(partial.print.grade, 90.0);
        assert_eq!(partial.print.toe, 6.0);
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let json = r#"{"schema_version": 7, "future": true, "print": {"grade": 60, "sparkle": 3}}"#;
        let s: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(s.schema_version, 7);
        assert_eq!(s.print.grade, 60.0);
    }

    #[test]
    fn channel_estimate_json_shape() {
        let auto = serde_json::to_value(ChannelEstimate::Auto).unwrap();
        assert_eq!(auto, serde_json::json!({"mode": "auto"}));
        let fixed = serde_json::to_value(ChannelEstimate::Fixed {
            value: [0.5, 0.25, 0.125],
        })
        .unwrap();
        assert_eq!(
            fixed,
            serde_json::json!({"mode": "fixed", "value": [0.5, 0.25, 0.125]})
        );
        let back: ChannelEstimate = serde_json::from_value(fixed).unwrap();
        assert_eq!(
            back,
            ChannelEstimate::Fixed {
                value: [0.5, 0.25, 0.125]
            }
        );
    }

    #[test]
    fn output_space_serialises_in_snake_case() {
        let cases = [
            (OutputSpace::Srgb, "\"srgb\""),
            (OutputSpace::AdobeRgb, "\"adobe_rgb\""),
            (OutputSpace::DisplayP3, "\"display_p3\""),
            (OutputSpace::ProPhoto, "\"pro_photo\""),
            (OutputSpace::Rec2020, "\"rec2020\""),
        ];
        for (space, text) in cases {
            assert_eq!(serde_json::to_string(&space).unwrap(), text);
            let back: OutputSpace = serde_json::from_str(text).unwrap();
            assert_eq!(back, space);
        }
    }

    #[test]
    fn content_hash_is_stable_and_sensitive() {
        let a = Settings::default();
        let b = Settings::default();
        assert_eq!(a.content_hash(), b.content_hash());

        let mut c = Settings::default();
        c.print.grade = f32::from_bits(c.print.grade.to_bits() + 1);
        assert_ne!(c.print.grade, a.print.grade);
        assert_ne!(c.content_hash(), a.content_hash());

        let mut d = Settings::default();
        d.film.gamma = ChannelEstimate::Fixed { value: [1.0; 3] };
        assert_ne!(d.content_hash(), a.content_hash());

        let mut r = RollSettings::default();
        let r0 = r.content_hash();
        r.pool_colour = false;
        assert_ne!(r.content_hash(), r0);
    }

    #[test]
    fn default_settings_hash_is_pinned() {
        // Changing the Settings schema (adding, removing, renaming or
        // reordering a field, or changing a default) or the hash encoding in
        // hash.rs legitimately changes this constant. Recompute it with
        // `Settings::default().content_hash().to_hex()` and paste the new
        // value here, and bump SETTINGS_SCHEMA_VERSION if sidecars change.
        assert_eq!(
            Settings::default().content_hash().to_hex(),
            "08ccdb441edc6cb36759df4583ae94c536cf75481a29703b1b40af8c6cc853b3"
        );
    }

    #[test]
    fn validate_checks_ranges() {
        Settings::default().validate().unwrap();
        RollSettings::default().validate().unwrap();

        let mut s = Settings::default();
        s.print.grade = 181.0;
        assert!(matches!(s.validate(), Err(Error::Invalid(_))));

        let mut s = Settings::default();
        s.print.grade = 49.9;
        assert!(matches!(s.validate(), Err(Error::Invalid(_))));

        let mut s = Settings::default();
        s.print.paper_dmax = 0.05;
        assert!(matches!(s.validate(), Err(Error::Invalid(_))));

        let mut s = Settings::default();
        s.print.shoulder = 0.0;
        assert!(matches!(s.validate(), Err(Error::Invalid(_))));

        let mut s = Settings::default();
        s.print.toe = -1.0;
        assert!(matches!(s.validate(), Err(Error::Invalid(_))));

        let mut s = Settings::default();
        s.print.exposure = f32::NAN;
        assert!(matches!(s.validate(), Err(Error::Invalid(_))));

        let mut s = Settings::default();
        s.print.cmy[1] = f32::INFINITY;
        assert!(matches!(s.validate(), Err(Error::Invalid(_))));

        let mut s = Settings::default();
        s.film.dmax = ChannelEstimate::Fixed {
            value: [1.0, f32::NAN, 1.0],
        };
        assert!(matches!(s.validate(), Err(Error::Invalid(_))));

        let mut r = RollSettings::default();
        r.shared.print.grade = 200.0;
        assert!(matches!(r.validate(), Err(Error::Invalid(_))));
    }

    #[test]
    fn validate_rejects_non_finite_print_floats_by_name() {
        let mut s = Settings::default();
        s.print.paper_dmax = f32::INFINITY;
        match s.validate() {
            Err(Error::Invalid(reason)) => {
                assert!(reason.contains("paper_dmax"), "{reason}");
            }
            other => panic!("expected Error::Invalid, got {other:?}"),
        }

        let mut s = Settings::default();
        s.print.grade = f32::NAN;
        match s.validate() {
            Err(Error::Invalid(reason)) => assert!(reason.contains("grade"), "{reason}"),
            other => panic!("expected Error::Invalid, got {other:?}"),
        }
    }
}
