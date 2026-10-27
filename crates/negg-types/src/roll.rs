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
use crate::geometry::Geometry;
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
    /// Overrides `print.saturation`. Omitted from the file when `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saturation: Option<f32>,
    /// Overrides `print.preset`. Omitted from the file when `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
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
/// Profiles are never overridden per frame. The geometry is overridden as a
/// whole (Ruling 71): a frame with its own crop gives its whole
/// [`Geometry`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FrameOverrides {
    /// Film model overrides (steps 3 and 4).
    pub film: FilmOverrides,
    /// Print stage overrides (step 5).
    pub print: PrintOverrides,
    /// Colour overrides (steps 6 and 7).
    pub colour: ColourOverrides,
    /// Replaces the roll's whole `geometry` when `Some` (Rulings 70 and
    /// 71). Omitted from the file when `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub geometry: Option<Geometry>,
}

impl FrameOverrides {
    /// True when no field overrides anything.
    pub fn is_empty(&self) -> bool {
        *self == FrameOverrides::default()
    }
}

/// The settings a frame is rendered with: the roll's shared settings with
/// each `Some` override of the frame replacing its field (R-lock), the
/// geometry as a whole (Ruling 71). The `profiles` always come from
/// `roll.shared`. A preset label override of `""` resolves to no label
/// (Ruling 8), so a frame can pin "no preset" over a roll that has one.
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
    if let Some(v) = p.saturation {
        s.print.saturation = v;
    }
    if let Some(v) = &p.preset {
        // An empty label override pins "no preset" (Ruling 8): preset
        // names are never empty, so "" cannot be a real label.
        s.print.preset = (!v.is_empty()).then(|| v.clone());
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
    if let Some(g) = overrides.geometry {
        s.geometry = g;
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
    /// The frame's Dmax in effect is pinned at the density cap
    /// ([`crate::estimates::dmax_pinned`], Ruling 133): its analysed area
    /// holds the film holder or another opaque area, so it was left out of
    /// the gamma and cast pools (its roles there are `Unused` unless the
    /// user fixed the value) and needs an analysis area. Omitted from the
    /// file when false; absent in older files, read as false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub needs_analysis_area: bool,
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
    /// Whether the pooled Dmin is measured on the clear film edge (rebate):
    /// at least two frames whose own Dmin came from the border are Dmin
    /// candidates (inliers or rejected), so the pool takes only those
    /// (step 3 of `docs/MATHS.md`, "Roll pooling"). False for a pool of
    /// percentile estimates, which an analysis area or a crop that leaves
    /// the rebate out makes (Ruling 138).
    pub fn dmin_from_border(&self) -> bool {
        self.frames
            .iter()
            .filter(|f| {
                matches!(f.dmin_role, PoolRole::Inlier | PoolRole::Rejected)
                    && f.dmin_source == DminSource::Border
            })
            .count()
            >= 2
    }

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

    fn crop_geometry() -> Geometry {
        Geometry {
            crop: Some(crate::geometry::CropRect {
                x: 0.1,
                y: 0.1,
                width: 0.8,
                height: 0.7,
            }),
            analysis_area: None,
            analysis_buffer: 0.0,
            rotate_degrees: 90,
            flip_horizontal: true,
        }
    }

    #[test]
    fn a_geometry_override_replaces_the_whole_geometry() {
        let mut r = roll();
        r.shared.geometry.crop = Some(crate::geometry::CropRect {
            x: 0.0,
            y: 0.0,
            width: 0.5,
            height: 0.5,
        });
        r.shared.geometry.rotate_degrees = 180;
        // Without an override the frame takes the roll's geometry.
        assert_eq!(
            resolve(&r, &FrameOverrides::default()).geometry,
            r.shared.geometry
        );
        let o = FrameOverrides {
            geometry: Some(crop_geometry()),
            ..Default::default()
        };
        let s = resolve(&r, &o);
        assert_eq!(s.geometry, crop_geometry());
        assert_eq!(s.print, r.shared.print);
        // The override round-trips and an empty one is omitted.
        let back: FrameOverrides =
            serde_json::from_str(&serde_json::to_string(&o).unwrap()).unwrap();
        assert_eq!(back, o);
        let empty = serde_json::to_value(FrameOverrides::default()).unwrap();
        assert!(empty.get("geometry").is_none(), "{empty}");
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
    fn resolve_takes_saturation_and_preset_overrides() {
        let r = RollSettings::default();
        let o = FrameOverrides {
            print: PrintOverrides {
                saturation: Some(1.3),
                preset: Some("punchy".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let s = resolve(&r, &o);
        assert_eq!(s.print.saturation, 1.3);
        assert_eq!(s.print.preset.as_deref(), Some("punchy"));
        let mut want = r.shared.clone();
        want.print.saturation = 1.3;
        want.print.preset = Some("punchy".into());
        assert_eq!(s, want);
        // No override inherits; an empty override set stays empty on disk.
        assert_eq!(resolve(&r, &FrameOverrides::default()), r.shared);
        let json = serde_json::to_string(&PrintOverrides::default()).unwrap();
        assert!(!json.contains("saturation") && !json.contains("preset"));
    }

    #[test]
    fn an_empty_label_override_pins_no_preset() {
        let mut r = RollSettings::default();
        r.shared.print.preset = Some("Rich".into());
        let o = FrameOverrides {
            print: PrintOverrides {
                preset: Some(String::new()),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(resolve(&r, &o).print.preset, None);
        // It is a look override: it counts as one and is cleared with the
        // rest.
        assert!(o.print.has_own_look());
        let mut cleared = o.print.clone();
        cleared.clear_look();
        assert_eq!(cleared, PrintOverrides::default());
        // It round-trips through JSON.
        let json = serde_json::to_string(&o).unwrap();
        assert_eq!(serde_json::from_str::<FrameOverrides>(&json).unwrap(), o);
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
            (
                FrameOverrides {
                    geometry: Some(crop_geometry()),
                    ..Default::default()
                },
                Box::new(|s| s.geometry = crop_geometry()),
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
                needs_analysis_area: false,
            }],
        };
        let json = serde_json::to_string(&pool).unwrap();
        assert!(json.contains("\"dmin_role\":\"inlier\""));
        assert!(json.contains("\"cast_role\":\"excluded\""));
        assert!(!json.contains("needs_analysis_area"), "omitted when false");
        let back: RollPool = serde_json::from_str(&json).unwrap();
        assert_eq!(back, pool);
        let mut pinned = pool.clone();
        pinned.frames[0].needs_analysis_area = true;
        let json = serde_json::to_string(&pinned).unwrap();
        assert!(json.contains("\"needs_analysis_area\":true"), "{json}");
        assert_eq!(serde_json::from_str::<RollPool>(&json).unwrap(), pinned);
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

    #[test]
    fn a_pool_is_border_sourced_with_two_border_candidates() {
        let frame = |source, role| PoolFrame {
            source: "f.dng".into(),
            dmin: [0.2, 0.5, 0.7],
            dmin_source: source,
            dmax: [1.9, 2.2, 2.4],
            gamma: [1.7; 3],
            cast: CastCorrection::IDENTITY,
            dmin_role: role,
            gamma_role: PoolRole::Inlier,
            cast_role: PoolRole::Inlier,
            needs_analysis_area: false,
        };
        let pool = |frames| RollPool {
            dmin: Some([0.2, 0.5, 0.7]),
            gamma: None,
            cast: None,
            frames,
        };
        use DminSource::{Border, Percentile};
        use PoolRole::{Excluded, Inlier, Rejected};
        assert!(pool(vec![frame(Border, Inlier), frame(Border, Rejected)]).dmin_from_border());
        assert!(!pool(vec![frame(Border, Inlier), frame(Percentile, Inlier)]).dmin_from_border());
        assert!(!pool(vec![frame(Border, Inlier), frame(Border, Excluded)]).dmin_from_border());
        assert!(
            !pool(vec![frame(Percentile, Inlier), frame(Percentile, Inlier)]).dmin_from_border()
        );
    }
}
