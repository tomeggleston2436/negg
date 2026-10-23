// SPDX-License-Identifier: MPL-2.0

//! Per-frame overrides of roll settings, and the roll pool record
//! (ADR 0017, R-pool, R-lock).
//!
//! A roll sidecar holds [`RollSettings`] and a [`RollPool`]; a frame's own
//! sidecar holds [`FrameOverrides`]. [`resolve`] combines the roll's shared
//! settings with a frame's overrides into the [`Settings`] that frame is
//! rendered with.

use serde::{Deserialize, Serialize};

use crate::estimates::{CastCorrection, DminSource};
use crate::settings::{CastEstimate, ChannelEstimate, OutputSpace, RollSettings, Settings};

/// Per-frame overrides of [`crate::settings::FilmSettings`]. `None` inherits.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FilmOverrides {
    /// Overrides `film.film_base`.
    pub film_base: Option<ChannelEstimate>,
    /// Overrides `film.dmax`.
    pub dmax: Option<ChannelEstimate>,
    /// Overrides `film.gamma`.
    pub gamma: Option<ChannelEstimate>,
}

/// Per-frame overrides of [`crate::settings::PrintSettings`]. `None` inherits.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrintOverrides {
    /// Overrides `print.grade`.
    pub grade: Option<f32>,
    /// Overrides `print.exposure`.
    pub exposure: Option<f32>,
    /// Overrides `print.cmy`.
    pub cmy: Option<[f32; 3]>,
    /// Overrides `print.paper_dmin`.
    pub paper_dmin: Option<f32>,
    /// Overrides `print.paper_dmax`.
    pub paper_dmax: Option<f32>,
    /// Overrides `print.shoulder`.
    pub shoulder: Option<f32>,
    /// Overrides `print.toe`.
    pub toe: Option<f32>,
    /// Overrides `print.black_point_compensation`.
    pub black_point_compensation: Option<bool>,
}

/// Per-frame overrides of [`crate::settings::ColourSettings`]. `None` inherits.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColourOverrides {
    /// Overrides `colour.auto_colour`.
    pub auto_colour: Option<bool>,
    /// Overrides `colour.cast`.
    pub cast: Option<CastEstimate>,
    /// Overrides `colour.output_space`.
    pub output_space: Option<OutputSpace>,
}

/// Everything a single frame overrides relative to its roll (R-lock). Every
/// field is optional: `None` means inherit from [`RollSettings::shared`].
/// Profiles are never overridden per frame.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FrameOverrides {
    /// Film model overrides (steps 3 and 4).
    pub film: FilmOverrides,
    /// Print stage overrides (step 5).
    pub print: PrintOverrides,
    /// Colour overrides (steps 6 and 7).
    pub colour: ColourOverrides,
}

impl FrameOverrides {
    /// True when no field overrides anything.
    pub fn is_empty(&self) -> bool {
        *self == FrameOverrides::default()
    }
}

/// The settings a frame is rendered with: the roll's shared settings with
/// each `Some` override of the frame replacing its field (R-lock). The
/// `profiles` always come from `roll.shared`.
pub fn resolve(roll: &RollSettings, overrides: &FrameOverrides) -> Settings {
    let mut s = roll.shared.clone();
    let f = &overrides.film;
    if let Some(v) = f.film_base {
        s.film.film_base = v;
    }
    if let Some(v) = f.dmax {
        s.film.dmax = v;
    }
    if let Some(v) = f.gamma {
        s.film.gamma = v;
    }
    let p = &overrides.print;
    if let Some(v) = p.grade {
        s.print.grade = v;
    }
    if let Some(v) = p.exposure {
        s.print.exposure = v;
    }
    if let Some(v) = p.cmy {
        s.print.cmy = v;
    }
    if let Some(v) = p.paper_dmin {
        s.print.paper_dmin = v;
    }
    if let Some(v) = p.paper_dmax {
        s.print.paper_dmax = v;
    }
    if let Some(v) = p.shoulder {
        s.print.shoulder = v;
    }
    if let Some(v) = p.toe {
        s.print.toe = v;
    }
    if let Some(v) = p.black_point_compensation {
        s.print.black_point_compensation = v;
    }
    let c = &overrides.colour;
    if let Some(v) = c.auto_colour {
        s.colour.auto_colour = v;
    }
    if let Some(v) = c.cast {
        s.colour.cast = v;
    }
    if let Some(v) = c.output_space {
        s.colour.output_space = v;
    }
    s
}

/// A frame's part in one pooled quantity (R-pool).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PoolRole {
    /// Contributed to the pooled value.
    Inlier,
    /// Was a candidate but lay too far from the pooled median.
    Rejected,
    /// Fixed by the user, so neither feeds nor takes the pool.
    Excluded,
    /// Not a candidate for this pool.
    Unused,
}

/// One frame's own estimates and pool roles, as recorded in the roll
/// sidecar.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PoolFrame {
    /// The source file name.
    pub source: String,
    /// The frame's own (unpooled) Dmin.
    pub dmin: [f32; 3],
    /// Where the frame's own Dmin came from.
    pub dmin_source: DminSource,
    /// The frame's own Dmax.
    pub dmax: [f32; 3],
    /// The frame's own gamma, as used for pooling.
    pub gamma: [f32; 3],
    /// The frame's own cast fit.
    pub cast: CastCorrection,
    /// Role in the Dmin pool.
    pub dmin_role: PoolRole,
    /// Role in the gamma pool.
    pub gamma_role: PoolRole,
    /// Role in the cast pool.
    pub cast_role: PoolRole,
}

/// The roll estimator's output: the pooled estimates and the per-frame
/// record they came from. `None` means no pooled value (R-pool).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RollPool {
    /// Pooled Dmin, if any frame qualified.
    pub dmin: Option<[f32; 3]>,
    /// Pooled gamma, if any frame qualified.
    pub gamma: Option<[f32; 3]>,
    /// Pooled cast, if any frame qualified.
    pub cast: Option<CastCorrection>,
    /// Every frame of the roll with its own estimates and roles.
    pub frames: Vec<PoolFrame>,
}

/// The pooled values a frame render may take, after the roll's `pool_*`
/// flags are applied.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PooledEstimates {
    /// Pooled Dmin, if pooling Dmin is on and a value exists.
    pub dmin: Option<[f32; 3]>,
    /// Pooled gamma, if pooling gamma is on and a value exists.
    pub gamma: Option<[f32; 3]>,
    /// Pooled cast, if pooling colour is on and a value exists.
    pub cast: Option<CastCorrection>,
}

impl RollPool {
    /// The pooled estimates, each `None` when its `pool_*` flag in `roll` is
    /// off.
    pub fn pooled(&self, roll: &RollSettings) -> PooledEstimates {
        PooledEstimates {
            dmin: if roll.pool_film_base { self.dmin } else { None },
            gamma: if roll.pool_gamma { self.gamma } else { None },
            cast: if roll.pool_colour { self.cast } else { None },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed(v: f32) -> ChannelEstimate {
        ChannelEstimate::Fixed { value: [v; 3] }
    }

    fn roll() -> RollSettings {
        let mut r = RollSettings::default();
        r.shared.print.grade = 100.0;
        r.shared.profiles.rig = Some("rigs/a.negg.toml".into());
        r
    }

    #[test]
    fn empty_object_loads_every_struct() {
        assert_eq!(
            serde_json::from_str::<FrameOverrides>("{}").unwrap(),
            FrameOverrides::default()
        );
        assert_eq!(
            serde_json::from_str::<FilmOverrides>("{}").unwrap(),
            FilmOverrides::default()
        );
        assert_eq!(
            serde_json::from_str::<PrintOverrides>("{}").unwrap(),
            PrintOverrides::default()
        );
        assert_eq!(
            serde_json::from_str::<ColourOverrides>("{}").unwrap(),
            ColourOverrides::default()
        );
        assert_eq!(
            serde_json::from_str::<RollPool>("{}").unwrap(),
            RollPool::default()
        );
        assert!(FrameOverrides::default().is_empty());
    }

    #[test]
    fn resolve_with_empty_overrides_equals_shared() {
        let r = roll();
        assert_eq!(resolve(&r, &FrameOverrides::default()), r.shared);
    }

    #[test]
    fn each_override_replaces_exactly_one_field() {
        let r = roll();
        let cast = CastEstimate::Fixed {
            slope_r: 1.1,
            offset_r: 0.01,
            slope_b: 0.9,
            offset_b: -0.01,
        };
        type Edit = Box<dyn Fn(&mut Settings)>;
        let cases: Vec<(FrameOverrides, Edit)> = vec![
            (
                FrameOverrides {
                    film: FilmOverrides {
                        film_base: Some(fixed(0.3)),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                Box::new(|s| s.film.film_base = fixed(0.3)),
            ),
            (
                FrameOverrides {
                    film: FilmOverrides {
                        dmax: Some(fixed(2.0)),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                Box::new(|s| s.film.dmax = fixed(2.0)),
            ),
            (
                FrameOverrides {
                    film: FilmOverrides {
                        gamma: Some(fixed(1.5)),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                Box::new(|s| s.film.gamma = fixed(1.5)),
            ),
            (
                FrameOverrides {
                    print: PrintOverrides {
                        grade: Some(90.0),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                Box::new(|s| s.print.grade = 90.0),
            ),
            (
                FrameOverrides {
                    print: PrintOverrides {
                        exposure: Some(0.5),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                Box::new(|s| s.print.exposure = 0.5),
            ),
            (
                FrameOverrides {
                    print: PrintOverrides {
                        cmy: Some([0.1, 0.0, -0.1]),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                Box::new(|s| s.print.cmy = [0.1, 0.0, -0.1]),
            ),
            (
                FrameOverrides {
                    print: PrintOverrides {
                        paper_dmin: Some(0.08),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                Box::new(|s| s.print.paper_dmin = 0.08),
            ),
            (
                FrameOverrides {
                    print: PrintOverrides {
                        paper_dmax: Some(2.2),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                Box::new(|s| s.print.paper_dmax = 2.2),
            ),
            (
                FrameOverrides {
                    print: PrintOverrides {
                        shoulder: Some(2.0),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                Box::new(|s| s.print.shoulder = 2.0),
            ),
            (
                FrameOverrides {
                    print: PrintOverrides {
                        toe: Some(5.0),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                Box::new(|s| s.print.toe = 5.0),
            ),
            (
                FrameOverrides {
                    print: PrintOverrides {
                        black_point_compensation: Some(true),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                Box::new(|s| s.print.black_point_compensation = true),
            ),
            (
                FrameOverrides {
                    colour: ColourOverrides {
                        auto_colour: Some(false),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                Box::new(|s| s.colour.auto_colour = false),
            ),
            (
                FrameOverrides {
                    colour: ColourOverrides {
                        cast: Some(cast),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                Box::new(move |s| s.colour.cast = cast),
            ),
            (
                FrameOverrides {
                    colour: ColourOverrides {
                        output_space: Some(OutputSpace::ProPhoto),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                Box::new(|s| s.colour.output_space = OutputSpace::ProPhoto),
            ),
        ];
        for (overrides, edit) in cases {
            assert!(!overrides.is_empty());
            let mut want = r.shared.clone();
            edit(&mut want);
            assert_ne!(want, r.shared, "{overrides:?} must change something");
            assert_eq!(resolve(&r, &overrides), want, "{overrides:?}");
        }
    }

    #[test]
    fn profiles_always_come_from_shared() {
        let r = roll();
        let s = resolve(&r, &FrameOverrides::default());
        assert_eq!(s.profiles.rig.as_deref(), Some("rigs/a.negg.toml"));
    }

    #[test]
    fn overrides_round_trip() {
        let o = FrameOverrides {
            film: FilmOverrides {
                gamma: Some(fixed(1.6)),
                ..Default::default()
            },
            colour: ColourOverrides {
                cast: Some(CastEstimate::Auto),
                ..Default::default()
            },
            ..Default::default()
        };
        let back: FrameOverrides =
            serde_json::from_str(&serde_json::to_string(&o).unwrap()).unwrap();
        assert_eq!(back, o);
    }

    #[test]
    fn pool_round_trips_and_respects_flags() {
        let pool = RollPool {
            dmin: Some([0.2, 0.5, 0.7]),
            gamma: Some([1.6, 1.7, 1.8]),
            cast: Some(CastCorrection {
                cells_used: 9,
                ..CastCorrection::IDENTITY
            }),
            frames: vec![PoolFrame {
                source: "a.dng".into(),
                dmin: [0.2, 0.5, 0.7],
                dmin_source: DminSource::Border,
                dmax: [1.9, 2.2, 2.4],
                gamma: [1.7; 3],
                cast: CastCorrection::IDENTITY,
                dmin_role: PoolRole::Inlier,
                gamma_role: PoolRole::Rejected,
                cast_role: PoolRole::Excluded,
            }],
        };
        let json = serde_json::to_string(&pool).unwrap();
        assert!(json.contains("\"dmin_role\":\"inlier\""));
        assert!(json.contains("\"cast_role\":\"excluded\""));
        let back: RollPool = serde_json::from_str(&json).unwrap();
        assert_eq!(back, pool);
        assert_eq!(
            serde_json::to_string(&PoolRole::Unused).unwrap(),
            "\"unused\""
        );

        let mut r = RollSettings::default();
        let all = pool.pooled(&r);
        assert_eq!(all.dmin, pool.dmin);
        assert_eq!(all.gamma, pool.gamma);
        assert_eq!(all.cast, pool.cast);
        r.pool_film_base = false;
        r.pool_gamma = false;
        r.pool_colour = false;
        assert_eq!(pool.pooled(&r), PooledEstimates::default());
        r.pool_gamma = true;
        let g = pool.pooled(&r);
        assert!(g.dmin.is_none() && g.cast.is_none() && g.gamma.is_some());
    }
}
