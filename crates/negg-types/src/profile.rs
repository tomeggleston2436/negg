// SPDX-License-Identifier: MPL-2.0

//! Serde forms of the rig and film calibration profiles (ADR 0014 to 0016).
//!
//! These are the data in a `*.negg.toml` profile file. This crate only
//! defines the structures and validates them; reading and writing TOML is
//! `negg-io`, and turning a profile into a runtime correction is
//! `RigProfile::density_model`, `FilmProfile::correction` and
//! `negg-calib`. Every struct rejects unknown fields and, apart from
//! `Option` fields, has no defaults: a profile is complete or it is wrong.
//!
//! Ground rule 6: no number in a committed profile without a measurement.
//! Every `source` text must say where its numbers came from, and
//! [`RigProfile::validate`] and [`FilmProfile::validate`] reject any that
//! still contain a `TODO(measure)` marker.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::rig::{DensityModel, FilmCorrection};

/// Schema version of profile files written and read by this build.
pub const PROFILE_SCHEMA_VERSION: u32 = 1;

/// The marker that flags a source as not yet measured.
const TODO_MARKER: &str = "TODO(measure)";

/// How a rig table was measured (step 2 and 3 of `docs/MATHS.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RigMethod {
    /// Flat field from a blank frame (R-flat).
    FlatField,
    /// Channel crosstalk from three narrow-band frames (R-crosstalk).
    Crosstalk,
    /// Density model fitted to a step wedge (R-density-fit).
    DensityFit,
}

/// Which density model a rig fit produced (R-density-fit).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DensityModelKind {
    /// Cross terms fitted as well as the diagonal.
    Full,
    /// Diagonal only: cross terms are zero.
    Diagonal,
}

/// How a film profile was measured (R-neutral-fit).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilmMethod {
    /// Neutral patches found in the frames of a roll.
    GreyCardRoll,
    /// A certified step wedge on this stock.
    StepWedge,
    /// A stock record with no measured correction.
    None,
}

/// A rig profile: the calibration of one capture setup (camera, light and
/// lens) that is independent of the film.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RigProfile {
    /// Must equal [`PROFILE_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// The profile's name, quoted by film profiles measured with it.
    pub name: String,
    /// Where the numbers came from: who measured, with what.
    pub source: String,
    /// Date of measurement, `YYYY-MM-DD`.
    pub measured_on: String,
    /// The tables present, exactly.
    pub method: Vec<RigMethod>,
    /// RMS residual of the density fit, in log-D; 0 without a fit.
    pub residual_rms: f32,
    /// The camera this rig was measured with, if recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<String>,
    /// Flat field table (step 2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flat_field: Option<FlatFieldTable>,
    /// Crosstalk table (step 2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crosstalk: Option<CrosstalkTable>,
    /// Density fit table (step 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub density_fit: Option<DensityFitTable>,
}

/// The flat field table: a reference to the flat frame and its summary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlatFieldTable {
    /// Must be [`RigMethod::FlatField`].
    pub method: RigMethod,
    /// Where the flat frame came from.
    pub source: String,
    /// Date of measurement, `YYYY-MM-DD`.
    pub measured_on: String,
    /// The flat frame, as a path relative to the profile file.
    pub frame: String,
    /// Mean of the unblurred grid block medians per channel (R-flat).
    pub mean: [f32; 3],
    /// How the blur width was chosen, for example `"long_side / 64"`.
    pub blur_sigma: String,
    /// Width of the flat frame in pixels; frames must match it exactly.
    pub width: u32,
    /// Height of the flat frame in pixels.
    pub height: u32,
}

/// The crosstalk table: three band frames and the matrix measured from them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrosstalkTable {
    /// Must be [`RigMethod::Crosstalk`].
    pub method: RigMethod,
    /// Where the band frames came from.
    pub source: String,
    /// Date of measurement, `YYYY-MM-DD`.
    pub measured_on: String,
    /// Red band frame, relative to the profile file.
    pub frame_r: String,
    /// Green band frame, relative to the profile file.
    pub frame_g: String,
    /// Blue band frame, relative to the profile file.
    pub frame_b: String,
    /// The crosstalk matrix `M` (column `j` is band `j`'s response); unit
    /// diagonal (R-crosstalk).
    pub m: [[f32; 3]; 3],
}

/// The density fit table (R-density-fit).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DensityFitTable {
    /// Must be [`RigMethod::DensityFit`].
    pub method: RigMethod,
    /// Where the wedge and its certified densities came from.
    pub source: String,
    /// Date of measurement, `YYYY-MM-DD`.
    pub measured_on: String,
    /// RMS of the fit residuals, in log-D.
    pub residual_rms: f32,
    /// Number of wedge patches fitted.
    pub n_patches: u32,
    /// Whether the cross terms were fitted.
    pub model: DensityModelKind,
    /// Per-channel offset `a_c`.
    pub a: [f32; 3],
    /// Per-channel diagonal slope `b_c = G_cc`; each must be negative.
    pub b: [f32; 3],
    /// Cross terms `c_ck = G_ck`; zero diagonal.
    pub c: [[f32; 3]; 3],
}

/// A film profile: the correction for one stock, measured with a rig.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilmProfile {
    /// Must equal [`PROFILE_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// The profile's name.
    pub name: String,
    /// Where the numbers came from.
    pub source: String,
    /// Date of measurement, `YYYY-MM-DD`.
    pub measured_on: String,
    /// How the profile was measured.
    pub method: FilmMethod,
    /// RMS residual of the neutral fit, in log-D.
    pub residual_rms: f32,
    /// The name of the rig profile used, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rig: Option<String>,
    /// The stock this profile is for.
    pub stock: StockTable,
    /// Roll-pooled film base (informational).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<BaseTable>,
    /// Roll-pooled Dmax (informational).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dmax: Option<DmaxTable>,
    /// Contrast ratios and offsets (applied at render).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gamma: Option<GammaTable>,
}

/// Identification of a film stock.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StockTable {
    /// The stock's name.
    pub name: String,
    /// The process; only `"C-41"` is supported.
    pub process: String,
    /// The box speed.
    pub iso: u32,
}

/// The film base measured on the roll (informational at render).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BaseTable {
    /// Dmin per channel, in log-D.
    pub dmin: [f32; 3],
    /// Where the value came from.
    pub source: String,
}

/// The maximum density measured on the roll (informational at render).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DmaxTable {
    /// Dmax per channel, in log-D.
    pub dmax: [f32; 3],
    /// Where the value came from.
    pub source: String,
}

/// The neutral-patch fit (R-neutral-fit, R-film-profile).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GammaTable {
    /// Absolute gamma at measurement (informational at render).
    pub gamma: [f32; 3],
    /// `gamma_c / gamma_G`; the green entry is 1.
    pub ratio: [f32; 3],
    /// Offset in log-D; the green entry is 0.
    pub offset: [f32; 3],
    /// How gamma was obtained, for example `"neutral_fit"`.
    pub gamma_source: String,
    /// Number of neutral patches fitted.
    pub n_patches: u32,
    /// Where the patches came from.
    pub source: String,
}

fn invalid(reason: String) -> Error {
    Error::Invalid(reason)
}

fn require(ok: bool, reason: impl FnOnce() -> String) -> Result<()> {
    if ok { Ok(()) } else { Err(invalid(reason())) }
}

/// A `source` text must be non-empty and must not be an unmeasured marker.
fn check_source(what: &str, source: &str) -> Result<()> {
    require(!source.trim().is_empty(), || {
        format!("{what} source must not be empty")
    })?;
    require(!source.contains(TODO_MARKER), || {
        format!(
            "{what} source contains {TODO_MARKER}: no number may be committed without a measurement"
        )
    })
}

/// `\d{4}-\d{2}-\d{2}`.
fn check_date(what: &str, date: &str) -> Result<()> {
    let b = date.as_bytes();
    let ok = b.len() == 10
        && b.iter().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                *c == b'-'
            } else {
                c.is_ascii_digit()
            }
        });
    require(ok, || {
        format!("{what} measured_on {date:?} must look like YYYY-MM-DD")
    })
}

fn check_finite(what: &str, values: &[f32]) -> Result<()> {
    require(values.iter().all(|v| v.is_finite()), || {
        format!("{what} {values:?} must be finite")
    })
}

fn check_finite3(what: &str, m: &[[f32; 3]; 3]) -> Result<()> {
    for row in m {
        check_finite(what, row)?;
    }
    Ok(())
}

fn check_schema(version: u32) -> Result<()> {
    require(version == PROFILE_SCHEMA_VERSION, || {
        format!("profile schema_version {version} is not {PROFILE_SCHEMA_VERSION}")
    })
}

impl RigProfile {
    /// Checks the profile against the rules of ADR 0014 and 0015.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Invalid`] when the schema version is not 1; a
    /// `source` is empty or contains `TODO(measure)`; a date is not
    /// `YYYY-MM-DD`; `method` is not exactly the set of tables present or a
    /// table's own `method` disagrees; any float is non-finite; the
    /// crosstalk diagonal is not 1 within 1e-4; the density cross-term
    /// diagonal is not 0; or any `b_c` is not negative.
    pub fn validate(&self) -> Result<()> {
        check_schema(self.schema_version)?;
        require(!self.name.trim().is_empty(), || {
            "rig name must not be empty".into()
        })?;
        check_source("rig", &self.source)?;
        check_date("rig", &self.measured_on)?;
        check_finite("residual_rms", &[self.residual_rms])?;

        let present = [
            (RigMethod::FlatField, self.flat_field.is_some()),
            (RigMethod::Crosstalk, self.crosstalk.is_some()),
            (RigMethod::DensityFit, self.density_fit.is_some()),
        ];
        for (method, has_table) in present {
            let listed = self.method.iter().filter(|m| **m == method).count();
            require(listed == usize::from(has_table), || {
                format!(
                    "method list {:?} must name {method:?} exactly once if and only if its table is present",
                    self.method
                )
            })?;
        }

        if let Some(t) = &self.flat_field {
            require(t.method == RigMethod::FlatField, || {
                "flat_field.method must be flat_field".into()
            })?;
            check_source("flat_field", &t.source)?;
            check_date("flat_field", &t.measured_on)?;
            require(!t.frame.trim().is_empty(), || {
                "flat_field.frame must not be empty".into()
            })?;
            require(!t.blur_sigma.trim().is_empty(), || {
                "flat_field.blur_sigma must not be empty".into()
            })?;
            check_finite("flat_field.mean", &t.mean)?;
            require(t.width > 0 && t.height > 0, || {
                "flat_field width and height must be positive".into()
            })?;
        }
        if let Some(t) = &self.crosstalk {
            require(t.method == RigMethod::Crosstalk, || {
                "crosstalk.method must be crosstalk".into()
            })?;
            check_source("crosstalk", &t.source)?;
            check_date("crosstalk", &t.measured_on)?;
            for (name, frame) in [
                ("frame_r", &t.frame_r),
                ("frame_g", &t.frame_g),
                ("frame_b", &t.frame_b),
            ] {
                require(!frame.trim().is_empty(), || {
                    format!("crosstalk.{name} must not be empty")
                })?;
            }
            check_finite3("crosstalk.m", &t.m)?;
            for (i, row) in t.m.iter().enumerate() {
                require((row[i] - 1.0).abs() <= 1e-4, || {
                    format!(
                        "crosstalk.m diagonal [{i}][{i}] = {} must be 1 within 1e-4",
                        row[i]
                    )
                })?;
            }
        }
        if let Some(t) = &self.density_fit {
            require(t.method == RigMethod::DensityFit, || {
                "density_fit.method must be density_fit".into()
            })?;
            check_source("density_fit", &t.source)?;
            check_date("density_fit", &t.measured_on)?;
            check_finite("density_fit.residual_rms", &[t.residual_rms])?;
            check_finite("density_fit.a", &t.a)?;
            check_finite("density_fit.b", &t.b)?;
            check_finite3("density_fit.c", &t.c)?;
            for i in 0..3 {
                require(t.c[i][i] == 0.0, || {
                    format!(
                        "density_fit.c diagonal [{i}][{i}] = {} must be 0",
                        t.c[i][i]
                    )
                })?;
                require(t.b[i] < 0.0, || {
                    format!("density_fit.b[{i}] = {} must be negative", t.b[i])
                })?;
            }
        }
        Ok(())
    }

    /// The runtime density model, `g = diag(b) + c`, or `None` when the
    /// profile has no density fit (R-density-fit).
    pub fn density_model(&self) -> Option<DensityModel> {
        let t = self.density_fit.as_ref()?;
        let mut g = t.c;
        for (i, row) in g.iter_mut().enumerate() {
            row[i] = t.b[i];
        }
        Some(DensityModel { a: t.a, g })
    }
}

impl FilmProfile {
    /// Checks the profile against the rules of ADR 0016.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Invalid`] when the schema version is not 1; a
    /// `source` is empty or contains `TODO(measure)`; a date is not
    /// `YYYY-MM-DD`; any float is non-finite; the process is not `"C-41"`
    /// or the ISO is zero; `ratio[1]` is not exactly 1, `offset[1]` not
    /// exactly 0 or a ratio is not positive; or `method` is `none` and any
    /// numeric table is present.
    pub fn validate(&self) -> Result<()> {
        check_schema(self.schema_version)?;
        require(!self.name.trim().is_empty(), || {
            "film name must not be empty".into()
        })?;
        check_source("film", &self.source)?;
        check_date("film", &self.measured_on)?;
        check_finite("residual_rms", &[self.residual_rms])?;
        require(self.stock.process == "C-41", || {
            format!("stock.process {:?} must be \"C-41\"", self.stock.process)
        })?;
        require(!self.stock.name.trim().is_empty(), || {
            "stock.name must not be empty".into()
        })?;
        require(self.stock.iso > 0, || "stock.iso must be positive".into())?;

        if self.method == FilmMethod::None {
            require(
                self.base.is_none() && self.dmax.is_none() && self.gamma.is_none(),
                || "method none forbids the base, dmax and gamma tables".into(),
            )?;
        }
        if let Some(t) = &self.base {
            check_source("base", &t.source)?;
            check_finite("base.dmin", &t.dmin)?;
        }
        if let Some(t) = &self.dmax {
            check_source("dmax", &t.source)?;
            check_finite("dmax.dmax", &t.dmax)?;
        }
        if let Some(t) = &self.gamma {
            check_source("gamma", &t.source)?;
            require(!t.gamma_source.trim().is_empty(), || {
                "gamma.gamma_source must not be empty".into()
            })?;
            check_finite("gamma.gamma", &t.gamma)?;
            check_finite("gamma.ratio", &t.ratio)?;
            check_finite("gamma.offset", &t.offset)?;
            require(t.ratio[1] == 1.0, || {
                format!("gamma.ratio[1] = {} must be exactly 1", t.ratio[1])
            })?;
            require(t.offset[1] == 0.0, || {
                format!("gamma.offset[1] = {} must be exactly 0", t.offset[1])
            })?;
            require(t.ratio.iter().all(|r| *r > 0.0), || {
                format!("gamma.ratio {:?} must be positive", t.ratio)
            })?;
        }
        Ok(())
    }

    /// The runtime film correction, or `None` when the profile has no
    /// `[gamma]` table or its method is `none` (R-film-profile).
    pub fn correction(&self) -> Option<FilmCorrection> {
        if self.method == FilmMethod::None {
            return None;
        }
        let g = self.gamma.as_ref()?;
        Some(FilmCorrection {
            name: self.name.clone(),
            rig: self.rig.clone(),
            gamma_ratio: g.ratio,
            offset_log_d: g.offset,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rig() -> RigProfile {
        RigProfile {
            schema_version: 1,
            name: "bench-a".into(),
            source: "measured on the bench, 2026-01-02".into(),
            measured_on: "2026-01-02".into(),
            method: vec![
                RigMethod::FlatField,
                RigMethod::Crosstalk,
                RigMethod::DensityFit,
            ],
            residual_rms: 0.004,
            camera: Some("X-T5".into()),
            flat_field: Some(FlatFieldTable {
                method: RigMethod::FlatField,
                source: "blank frame".into(),
                measured_on: "2026-01-02".into(),
                frame: "flat.tif".into(),
                mean: [0.4, 0.5, 0.3],
                blur_sigma: "long_side / 64".into(),
                width: 6000,
                height: 4000,
            }),
            crosstalk: Some(CrosstalkTable {
                method: RigMethod::Crosstalk,
                source: "three bands".into(),
                measured_on: "2026-01-02".into(),
                frame_r: "r.tif".into(),
                frame_g: "g.tif".into(),
                frame_b: "b.tif".into(),
                m: [[1.0, 0.05, 0.0], [0.1, 1.0, 0.1], [0.0, 0.02, 1.0]],
            }),
            density_fit: Some(DensityFitTable {
                method: RigMethod::DensityFit,
                source: "certified wedge".into(),
                measured_on: "2026-01-03".into(),
                residual_rms: 0.004,
                n_patches: 12,
                model: DensityModelKind::Full,
                a: [0.01, 0.02, 0.03],
                b: [-1.0, -0.95, -1.05],
                c: [[0.0, 0.02, 0.0], [0.01, 0.0, 0.01], [0.0, 0.03, 0.0]],
            }),
        }
    }

    fn film() -> FilmProfile {
        FilmProfile {
            schema_version: 1,
            name: "portra-400-bench-a".into(),
            source: "grey card, roll 12".into(),
            measured_on: "2026-02-03".into(),
            method: FilmMethod::GreyCardRoll,
            residual_rms: 0.01,
            rig: Some("bench-a".into()),
            stock: StockTable {
                name: "Portra 400".into(),
                process: "C-41".into(),
                iso: 400,
            },
            base: Some(BaseTable {
                dmin: [0.2, 0.5, 0.7],
                source: "roll pool".into(),
            }),
            dmax: Some(DmaxTable {
                dmax: [1.9, 2.2, 2.4],
                source: "roll pool".into(),
            }),
            gamma: Some(GammaTable {
                gamma: [1.7, 1.65, 1.6],
                ratio: [1.03, 1.0, 0.97],
                offset: [0.01, 0.0, -0.02],
                gamma_source: "neutral_fit".into(),
                n_patches: 3,
                source: "grey card".into(),
            }),
        }
    }

    fn rejected(r: Result<()>) {
        assert!(matches!(r, Err(Error::Invalid(_))), "{r:?}");
    }

    #[test]
    fn profiles_round_trip_through_json() {
        let r = rig();
        let back: RigProfile = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(back, r);
        let f = film();
        let back: FilmProfile = serde_json::from_str(&serde_json::to_string(&f).unwrap()).unwrap();
        assert_eq!(back, f);
        // Option fields are omitted when absent.
        let mut bare = rig();
        bare.camera = None;
        bare.flat_field = None;
        bare.crosstalk = None;
        bare.density_fit = None;
        bare.method.clear();
        let json = serde_json::to_string(&bare).unwrap();
        assert!(
            !json.contains("camera") && !json.contains("flat_field"),
            "{json}"
        );
        let back: RigProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(back, bare);
    }

    #[test]
    fn enums_serialise_in_snake_case() {
        assert_eq!(
            serde_json::to_string(&RigMethod::FlatField).unwrap(),
            "\"flat_field\""
        );
        assert_eq!(
            serde_json::to_string(&RigMethod::DensityFit).unwrap(),
            "\"density_fit\""
        );
        assert_eq!(
            serde_json::to_string(&DensityModelKind::Diagonal).unwrap(),
            "\"diagonal\""
        );
        assert_eq!(
            serde_json::to_string(&FilmMethod::GreyCardRoll).unwrap(),
            "\"grey_card_roll\""
        );
        assert_eq!(
            serde_json::to_string(&FilmMethod::StepWedge).unwrap(),
            "\"step_wedge\""
        );
        assert_eq!(
            serde_json::to_string(&FilmMethod::None).unwrap(),
            "\"none\""
        );
    }

    #[test]
    fn unknown_and_missing_fields_are_rejected() {
        assert!(serde_json::from_str::<RigProfile>("{}").is_err());
        assert!(serde_json::from_str::<FilmProfile>("{}").is_err());
        let mut v = serde_json::to_value(film()).unwrap();
        v["sparkle"] = serde_json::json!(1);
        assert!(serde_json::from_value::<FilmProfile>(v).is_err());
        let mut v = serde_json::to_value(film()).unwrap();
        v["stock"]["sparkle"] = serde_json::json!(1);
        assert!(serde_json::from_value::<FilmProfile>(v).is_err());
    }

    #[test]
    fn valid_profiles_validate() {
        rig().validate().unwrap();
        film().validate().unwrap();
        let mut none = film();
        none.method = FilmMethod::None;
        none.base = None;
        none.dmax = None;
        none.gamma = None;
        none.validate().unwrap();
        assert!(none.correction().is_none());
    }

    #[test]
    fn validate_rejects_todo_measure_and_empty_sources() {
        let mut r = rig();
        r.source = "TODO(measure) bench".into();
        rejected(r.validate());
        let mut r = rig();
        r.crosstalk.as_mut().unwrap().source = "x TODO(measure)".into();
        rejected(r.validate());
        let mut r = rig();
        r.source = "  ".into();
        rejected(r.validate());
        let mut f = film();
        f.gamma.as_mut().unwrap().source = "TODO(measure)".into();
        rejected(f.validate());
        let mut f = film();
        f.base.as_mut().unwrap().source = String::new();
        rejected(f.validate());
    }

    #[test]
    fn validate_rejects_bad_dates() {
        for bad in [
            "2026-1-02",
            "26-01-02",
            "2026/01/02",
            "2026-01-0x",
            "",
            "2026-01-022",
        ] {
            let mut r = rig();
            r.measured_on = bad.into();
            rejected(r.validate());
            let mut f = film();
            f.measured_on = bad.into();
            rejected(f.validate());
        }
        let mut r = rig();
        r.density_fit.as_mut().unwrap().measured_on = "yesterday".into();
        rejected(r.validate());
    }

    #[test]
    fn validate_rejects_wrong_method_lists() {
        let mut r = rig();
        r.method.pop();
        rejected(r.validate());
        let mut r = rig();
        r.method.push(RigMethod::FlatField);
        rejected(r.validate());
        let mut r = rig();
        r.flat_field = None;
        rejected(r.validate());
        let mut r = rig();
        r.crosstalk.as_mut().unwrap().method = RigMethod::FlatField;
        rejected(r.validate());
        let mut r = rig();
        r.density_fit.as_mut().unwrap().method = RigMethod::Crosstalk;
        rejected(r.validate());
    }

    #[test]
    fn validate_rejects_positive_b_and_bad_matrices() {
        for i in 0..3 {
            let mut r = rig();
            r.density_fit.as_mut().unwrap().b[i] = 0.0;
            rejected(r.validate());
            let mut r = rig();
            r.density_fit.as_mut().unwrap().b[i] = 0.5;
            rejected(r.validate());
            let mut r = rig();
            r.density_fit.as_mut().unwrap().c[i][i] = 0.1;
            rejected(r.validate());
            let mut r = rig();
            r.crosstalk.as_mut().unwrap().m[i][i] = 1.001;
            rejected(r.validate());
        }
        let mut r = rig();
        r.crosstalk.as_mut().unwrap().m[0][0] = 1.00005;
        r.validate().unwrap();
        let mut r = rig();
        r.density_fit.as_mut().unwrap().a[1] = f32::NAN;
        rejected(r.validate());
        let mut r = rig();
        r.flat_field.as_mut().unwrap().mean[0] = f32::INFINITY;
        rejected(r.validate());
        let mut r = rig();
        r.schema_version = 2;
        rejected(r.validate());
    }

    #[test]
    fn film_validate_rules() {
        let mut f = film();
        f.gamma.as_mut().unwrap().ratio[1] = 1.0001;
        rejected(f.validate());
        let mut f = film();
        f.gamma.as_mut().unwrap().offset[1] = 0.001;
        rejected(f.validate());
        let mut f = film();
        f.gamma.as_mut().unwrap().ratio[0] = 0.0;
        rejected(f.validate());
        let mut f = film();
        f.gamma.as_mut().unwrap().offset[2] = f32::NAN;
        rejected(f.validate());
        let mut f = film();
        f.stock.process = "E-6".into();
        rejected(f.validate());
        let mut f = film();
        f.method = FilmMethod::None;
        rejected(f.validate());
        let mut f = film();
        f.schema_version = 0;
        rejected(f.validate());
    }

    #[test]
    fn density_model_is_diag_b_plus_c() {
        let m = rig().density_model().unwrap();
        assert_eq!(m.a, [0.01, 0.02, 0.03]);
        assert_eq!(m.g[0], [-1.0, 0.02, 0.0]);
        assert_eq!(m.g[1], [0.01, -0.95, 0.01]);
        assert_eq!(m.g[2], [0.0, 0.03, -1.05]);
        let mut r = rig();
        r.density_fit = None;
        assert!(r.density_model().is_none());
    }

    #[test]
    fn correction_follows_the_gamma_table() {
        let c = film().correction().unwrap();
        assert_eq!(c.name, "portra-400-bench-a");
        assert_eq!(c.rig.as_deref(), Some("bench-a"));
        assert_eq!(c.gamma_ratio, [1.03, 1.0, 0.97]);
        assert_eq!(c.offset_log_d, [0.01, 0.0, -0.02]);
        let mut f = film();
        f.gamma = None;
        assert!(f.correction().is_none());
        let mut f = film();
        f.method = FilmMethod::None;
        assert!(f.correction().is_none());
    }
}
