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
pub const SETTINGS_SCHEMA_VERSION: u32 = 2;

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

/// The step 6 cast correction: fitted automatically or fixed by the user.
///
/// Serialises as `{"mode": "auto"}` or
/// `{"mode": "fixed", "slope_r": .., "offset_r": .., "slope_b": .., "offset_b": ..}`.
/// A fixed cast always takes effect, with `auto_colour` on or off: the
/// precedence is Fixed, then the identity when `auto_colour` is off or a
/// film correction is in use, then the roll's pooled cast, then the fit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum CastEstimate {
    /// Fit the cast on the analysis grid (or pool it across the roll).
    #[default]
    Auto,
    /// Use this log-affine correction, locked or overridden by the user.
    Fixed {
        /// Slope of `e_R` against `e_G`; valid range 0.5 to 2.
        slope_r: f32,
        /// Offset of `e_R` against `e_G`.
        offset_r: f32,
        /// Slope of `e_B` against `e_G`; valid range 0.5 to 2.
        slope_b: f32,
        /// Offset of `e_B` against `e_G`.
        offset_b: f32,
    },
}

/// Names of the rig and film profile files a frame or roll is converted
/// with (phase 2, ADR 0018).
///
/// Paths resolve relative to the directory of the file that holds them;
/// command line paths are made absolute before they are stored here.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfileRefs {
    /// Path of the rig profile (`*.negg.toml`), if any.
    pub rig: Option<String>,
    /// Path of the film profile (`*.negg.toml`), if any.
    pub film: Option<String>,
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
    /// Step 6 of `docs/MATHS.md`: the cast correction, fitted (`Auto`, the
    /// default) or fixed by the user.
    pub cast: CastEstimate,
    /// Step 7 of `docs/MATHS.md`: the output colour space. Default sRGB.
    pub output_space: OutputSpace,
}

impl Default for ColourSettings {
    fn default() -> Self {
        ColourSettings {
            auto_colour: true,
            cast: CastEstimate::Auto,
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
    /// Rig and film profiles to apply (phase 2). Default none.
    pub profiles: ProfileRefs,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            schema_version: SETTINGS_SCHEMA_VERSION,
            film: FilmSettings::default(),
            print: PrintSettings::default(),
            colour: ColourSettings::default(),
            profiles: ProfileRefs::default(),
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
    /// toe is not positive, any fixed channel value is non-finite, or a fixed
    /// cast has a slope outside 0.5 to 2 or a non-finite offset.
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
        if let CastEstimate::Fixed {
            slope_r,
            offset_r,
            slope_b,
            offset_b,
        } = self.colour.cast
        {
            for (name, slope) in [("slope_r", slope_r), ("slope_b", slope_b)] {
                require((0.5..=2.0).contains(&slope), || {
                    format!("fixed cast {name} {slope} must be between 0.5 and 2")
                })?;
            }
            for (name, offset) in [("offset_r", offset_r), ("offset_b", offset_b)] {
                require(offset.is_finite(), || {
                    format!("fixed cast {name} {offset} must be finite")
                })?;
            }
        }
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
            "0aef82b17d60343045c4b0b09c3a61a527e7fdb902b1f9b57c966def5efb5202"
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

    fn fixed_cast() -> CastEstimate {
        CastEstimate::Fixed {
            slope_r: 1.1,
            offset_r: 0.02,
            slope_b: 0.9,
            offset_b: -0.01,
        }
    }

    #[test]
    fn phase2_defaults_are_identity() {
        let s = Settings::default();
        assert_eq!(s.schema_version, 2);
        assert_eq!(s.colour.cast, CastEstimate::Auto);
        assert_eq!(s.profiles, ProfileRefs::default());
        assert!(s.profiles.rig.is_none() && s.profiles.film.is_none());
        assert_eq!(CastEstimate::default(), CastEstimate::Auto);
    }

    #[test]
    fn new_settings_types_round_trip() {
        let mut s = Settings::default();
        s.colour.cast = fixed_cast();
        s.profiles = ProfileRefs {
            rig: Some("rigs/a.negg.toml".into()),
            film: Some("/abs/film.negg.toml".into()),
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
        assert_eq!(
            serde_json::to_value(CastEstimate::Auto).unwrap(),
            serde_json::json!({"mode": "auto"})
        );
        assert_eq!(
            serde_json::to_value(fixed_cast()).unwrap()["mode"],
            serde_json::json!("fixed")
        );
        let refs: ProfileRefs = serde_json::from_str("{}").unwrap();
        assert_eq!(refs, ProfileRefs::default());
    }

    #[test]
    fn v1_settings_json_still_loads() {
        let v1 = r#"{
            "schema_version": 1,
            "film": {"film_base": {"mode": "auto"}, "dmax": {"mode": "auto"},
                     "gamma": {"mode": "auto"}},
            "print": {"grade": 115.0, "exposure": 0.0, "cmy": [0.0, 0.0, 0.0],
                      "paper_dmin": 0.06, "paper_dmax": 2.3, "shoulder": 3.0,
                      "toe": 6.0, "black_point_compensation": false},
            "colour": {"auto_colour": true, "output_space": "srgb"}
        }"#;
        let s: Settings = serde_json::from_str(v1).unwrap();
        assert_eq!(s.schema_version, 1, "the file's own version is preserved");
        assert_eq!(s.colour.cast, CastEstimate::Auto);
        assert_eq!(s.profiles, ProfileRefs::default());
        let current = Settings {
            schema_version: 1,
            ..Settings::default()
        };
        assert_eq!(s, current);
        s.validate().unwrap();
        let r: RollSettings =
            serde_json::from_str(r#"{"schema_version": 1, "shared": {"schema_version": 1}}"#)
                .unwrap();
        assert_eq!(r.shared.colour.cast, CastEstimate::Auto);
    }

    #[test]
    fn validate_checks_fixed_cast() {
        let mut s = Settings::default();
        s.colour.cast = fixed_cast();
        s.validate().unwrap();
        for (slope_r, offset_r, slope_b, offset_b) in [
            (0.49, 0.0, 1.0, 0.0),
            (2.01, 0.0, 1.0, 0.0),
            (1.0, 0.0, 0.4, 0.0),
            (1.0, 0.0, 2.5, 0.0),
            (f32::NAN, 0.0, 1.0, 0.0),
            (1.0, f32::NAN, 1.0, 0.0),
            (1.0, 0.0, 1.0, f32::INFINITY),
        ] {
            s.colour.cast = CastEstimate::Fixed {
                slope_r,
                offset_r,
                slope_b,
                offset_b,
            };
            assert!(matches!(s.validate(), Err(Error::Invalid(_))), "{s:?}");
        }
        s.colour.cast = CastEstimate::Fixed {
            slope_r: 0.5,
            offset_r: 0.0,
            slope_b: 2.0,
            offset_b: 0.0,
        };
        s.validate().unwrap();
    }
}
