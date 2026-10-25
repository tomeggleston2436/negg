// SPDX-License-Identifier: MPL-2.0

//! Roll mode: pass 1 of the two-pass roll render (ADR 0017, R-roll).
//!
//! [`analyse_frame`] reduces one frame to what the roll estimator needs
//! (step 3 of `docs/MATHS.md`: the analysis grid density, Dmin and Dmax),
//! and [`pool_roll`] pools Dmin and gamma across the frames (step 3, "Roll
//! pooling", R-pool) and then the cast fits (step 6, "Roll mode",
//! R-cast-pool). Pass 2 renders each frame with
//! [`crate::render_frame_with`] and the pooled values in its
//! [`crate::RenderContext`]; the precedence of fixed settings, film
//! correction, pool and frame estimate is applied there (R-lock).
//!
//! The pooling statistics themselves live in `negg-film`
//! ([`dmin_candidates`], [`pool_channels`], [`pool_cast`]); this module only
//! decides each frame's candidacy and role and feeds them in order.

use std::borrow::Cow;

use negg_film::{
    AnalysisGrid, DminEstimate, analysis_grid, dmin_candidates, no_profile_gamma, pool_cast,
    pool_channels,
};
use negg_types::settings::ChannelEstimate;
use negg_types::{
    CastCorrection, CastEstimate, DensityModel, DminSource, Error, FilmCorrection, Geometry, Image,
    PoolFrame, PoolRole, Result, RollPool, RollSettings, Settings, dmax_pinned,
};

use rayon::prelude::*;

use crate::cast::fit_cast;
use crate::invert::invert;
use crate::pipeline::{ANALYSIS_GRID_LONG_SIDE, analysis_area, area_dmin_dmax, dmin_with_pool};
use crate::rig::density_grid;

/// What pass 1 keeps of one frame: the step 3 density of its analysis grid
/// and its own Dmin and Dmax estimates. The grid itself is dropped, so
/// about 8 MB is retained per frame (a 1024 by 683 grid of three `f32`).
#[derive(Clone, Debug, PartialEq)]
pub struct FrameAnalysis {
    /// The density of the analysis grid (step 3), with the rig density
    /// model when one was given: the whole grid without an analysis area or
    /// a crop, else the buffered analysis grid (Rulings 90 and 134), as the
    /// cast pool uses it.
    pub grid_density: Image,
    /// The frame's own Dmin estimate and its source (Border or Percentile).
    pub dmin: DminEstimate,
    /// The frame's own Dmax estimate.
    pub dmax: [f32; 3],
}

/// Step 3 of `docs/MATHS.md` for one frame of a roll from its cached
/// whole-frame analysis grid and grid density (Rulings 90 and 91): the
/// frame's own Dmin and Dmax estimated exactly as
/// [`crate::estimate_frame`] does under `geometry` (Dmin on the unbuffered
/// analysis rectangle's grid, Dmax on the buffered analysis grid, a rig
/// density model honoured), and the grid density the cast pool fits on
/// (the analysis grid's part). The analysis rectangle is the analysis area
/// when set, else the crop (Ruling 134); with neither the whole grid is
/// used and the analysis buffer is ignored.
///
/// # Errors
///
/// [`Error::Invalid`] when the grid has no cells, `grid_density` does not
/// match it, the geometry fails validation, or a crop leaves fewer than
/// [`crate::MIN_ANALYSIS_CELLS`] analysis cells.
///
/// `grid_density` may be borrowed or owned: the analysis rectangle is
/// worked out first, and the density is copied only when there is none
/// and it was borrowed (the result keeps the whole grid density then).
pub fn analyse_grid<'a>(
    grid: &AnalysisGrid,
    grid_density: impl Into<Cow<'a, Image>>,
    geometry: &Geometry,
    density: Option<&DensityModel>,
) -> Result<FrameAnalysis> {
    let grid_density = grid_density.into();
    if grid.cells() == 0 {
        return Err(Error::Invalid(
            "analysis grid has no cells: nothing to analyse".to_string(),
        ));
    }
    if grid_density.width() != grid.width() || grid_density.height() != grid.height() {
        return Err(Error::Invalid(format!(
            "grid density is {}x{} but the analysis grid is {}x{}",
            grid_density.width(),
            grid_density.height(),
            grid.width(),
            grid.height()
        )));
    }
    let area = analysis_area(grid, &grid_density, geometry)?;
    let (dmin, dmax) = area_dmin_dmax(grid, &grid_density, area.as_ref(), density);
    let grid_density = match area {
        Some(a) => a.analysis_density,
        None => grid_density.into_owned(),
    };
    Ok(FrameAnalysis {
        grid_density,
        dmin,
        dmax,
    })
}

/// Step 3 of `docs/MATHS.md` for one frame of a roll: the analysis grid of
/// `linear` (after the caller's step 2, [`crate::apply_rig_linear`]), its
/// density with the rig model `density` when given (R-density-use), then
/// [`analyse_grid`] under `geometry`.
///
/// # Errors
///
/// As [`analyse_grid`]; an image with no pixels has no grid cells.
pub fn analyse_frame(
    linear: &Image,
    density: Option<&DensityModel>,
    geometry: &Geometry,
) -> Result<FrameAnalysis> {
    let grid = analysis_grid(linear, ANALYSIS_GRID_LONG_SIDE);
    let grid_density = density_grid(&grid.median, density);
    analyse_grid(&grid, Cow::Owned(grid_density), geometry, density)
}

/// One frame as [`pool_roll`] sees it: its file name, its pass 1 analysis
/// and the settings it resolves to (the roll's shared settings with the
/// frame's overrides, R-lock).
#[derive(Clone, Copy, Debug)]
pub struct RollFrame<'a> {
    /// The source file name, recorded in [`PoolFrame::source`].
    pub source: &'a str,
    /// The frame's pass 1 analysis.
    pub analysis: &'a FrameAnalysis,
    /// The frame's resolved settings.
    pub settings: &'a Settings,
}

fn fixed(estimate: ChannelEstimate) -> Option<[f32; 3]> {
    match estimate {
        ChannelEstimate::Fixed { value } => Some(value),
        ChannelEstimate::Auto => None,
    }
}

/// Re-marks the frames the user fixed as [`PoolRole::Excluded`].
fn mark_excluded(mut roles: Vec<PoolRole>, excluded: &[bool]) -> Vec<PoolRole> {
    for (role, &ex) in roles.iter_mut().zip(excluded) {
        if ex {
            *role = PoolRole::Excluded;
        }
    }
    roles
}

/// Steps 3 and 6 of `docs/MATHS.md` across a roll: pools Dmin, gamma and the
/// cast over `frames` (R-pool, R-cast-pool, R-lock; ADR 0017).
///
/// - Dmin: a frame whose resolved `film.film_base` is Fixed is Excluded.
///   The candidates are chosen by [`dmin_candidates`] (Border frames when
///   at least two non-excluded frames are Border) and pooled by
///   [`pool_channels`].
/// - Dmin in effect: Fixed, else `dmin_with_pool`
///   of its own estimate and the pooled Dmin (the pooled value, capped per
///   channel by the frame's own estimate when that is a percentile one,
///   Ruling 105), exactly as [`crate::estimate_frame`] renders it.
/// - Gamma: per frame `gamma_f = max(Dmax_f - Dmin_f, 0.05)` with `Dmin_f`
///   the frame's Dmin in effect and `Dmax_f` its Dmax in effect (Fixed,
///   else its own). A frame whose `film.gamma` is Fixed is Excluded; every
///   other frame is a candidate.
/// - Cast: a frame with `colour.cast` Fixed or `auto_colour` off is
///   Excluded. Every other frame's cast is fitted with [`fit_cast`] on its
///   grid density inverted with its Dmin and gamma in effect (Fixed, else
///   pooled, else its own). The candidates are the frames not Rejected for
///   Dmin or gamma; a frame Rejected there is Rejected here too. Pooled by
///   [`pool_cast`] (weight `cells_used`). With a film correction the cast is
///   the identity (R-contrast), so nothing is fitted or pooled: the cast
///   pool is `None`, user-fixed frames are Excluded and the rest Unused.
///   The cast is pooled only when the pooled Dmin came from Border frames
///   (every Dmin candidate is Border, Ruling 106); otherwise the fits are
///   kept in the record but not pooled: the cast pool is `None`, user-fixed
///   frames are Excluded and the rest Unused, and each frame is rendered
///   with its own fit.
/// - A `pool_*` flag of `roll` that is off gives that pool `None` with
///   every role Unused.
/// - Ruling 133: a frame whose Dmax in effect is pinned at the density cap
///   ([`negg_types::dmax_pinned`]: the film holder or another opaque area
///   is in its analysed rectangle) is not a candidate for the gamma pool
///   nor the cast pool: its role there is Unused (Excluded when the user
///   fixed the value), and its record has `needs_analysis_area` set
///   whatever the pool flags. Its Dmin still pools as any other frame's
///   (the film base is unaffected by an opaque surround). When every frame
///   is pinned there is no pooled gamma (and no pooled cast), so each frame
///   keeps its own. Pass 2 renders a pinned frame like any other frame,
///   with the pooled values when there are some: the pooled gamma and cast
///   come from the good frames of the same film, a better estimate for it
///   than its own pinned range. Its exposure anchor and `x_0` (step 5), and
///   its own cast fit when there is no cast pool, still come from its
///   grid, holder cells included, so it may still print off: the
///   `needs_analysis_area` flag (the app's badge) is the remedy.
///
/// [`PoolFrame`] records each frame's own values: Dmin (its Fixed value
/// with source Fixed when Excluded), Dmax in effect, `gamma_f` (or its
/// Fixed value) and its own cast fit (its Fixed cast, or the identity when
/// excluded or not fitted).
///
/// # Errors
///
/// [`negg_types::Error::Invalid`] when a frame's settings fail validation
/// or its grid density cannot be inverted (a non-finite Dmin or a gamma
/// that is not finite and positive).
pub fn pool_roll(
    frames: &[RollFrame],
    roll: &RollSettings,
    film: Option<&FilmCorrection>,
) -> Result<RollPool> {
    for f in frames {
        f.settings.validate()?;
    }
    let n = frames.len();

    // Step 3: Dmin.
    let dmin_fixed: Vec<Option<[f32; 3]>> = frames
        .iter()
        .map(|f| fixed(f.settings.film.film_base))
        .collect();
    let dmin_excluded: Vec<bool> = dmin_fixed.iter().map(Option::is_some).collect();
    let own_dmin: Vec<[f32; 3]> = frames
        .iter()
        .zip(&dmin_fixed)
        .map(|(f, fx)| fx.unwrap_or(f.analysis.dmin.value))
        .collect();
    let (pooled_dmin, dmin_roles) = if roll.pool_film_base {
        let sources: Vec<DminSource> = frames.iter().map(|f| f.analysis.dmin.source).collect();
        let candidates = dmin_candidates(&sources, &dmin_excluded);
        let pool = pool_channels(&own_dmin, &candidates);
        (pool.value, mark_excluded(pool.roles, &dmin_excluded))
    } else {
        (None, vec![PoolRole::Unused; n])
    };
    // The Dmin each frame renders with (Ruling 105): exactly what
    // `estimate_frame` takes, so the gamma pooled below and the cast fits
    // use the same base as the render.
    let dmin_in_effect: Vec<[f32; 3]> = frames
        .iter()
        .zip(&dmin_fixed)
        .map(|(f, fx)| fx.unwrap_or_else(|| dmin_with_pool(&f.analysis.dmin, pooled_dmin).0))
        .collect();
    // Ruling 106: the cast is pooled only when the pooled Dmin came from
    // Border frames, that is every Dmin candidate (Inlier or Rejected) is
    // a Border frame, as the border preference of `dmin_candidates` makes
    // it when at least two non-excluded frames are Border.
    let border_pool = pooled_dmin.is_some() && {
        let mut candidates = (0..n)
            .filter(|&i| matches!(dmin_roles[i], PoolRole::Inlier | PoolRole::Rejected))
            .peekable();
        candidates.peek().is_some()
            && candidates.all(|i| frames[i].analysis.dmin.source == DminSource::Border)
    };
    let dmax_in_effect: Vec<[f32; 3]> = frames
        .iter()
        .map(|f| fixed(f.settings.film.dmax).unwrap_or(f.analysis.dmax))
        .collect();
    // Ruling 133: a Dmax pinned at the density cap keeps the frame out of
    // the gamma and cast pools.
    let pinned: Vec<bool> = dmax_in_effect.iter().map(dmax_pinned).collect();

    // Step 3: gamma.
    let gamma_fixed: Vec<Option<[f32; 3]>> = frames
        .iter()
        .map(|f| fixed(f.settings.film.gamma))
        .collect();
    let gamma_excluded: Vec<bool> = gamma_fixed.iter().map(Option::is_some).collect();
    let own_gamma: Vec<[f32; 3]> = (0..n)
        .map(|i| {
            gamma_fixed[i].unwrap_or_else(|| no_profile_gamma(dmin_in_effect[i], dmax_in_effect[i]))
        })
        .collect();
    let (pooled_gamma, gamma_roles) = if roll.pool_gamma {
        let candidates: Vec<bool> = (0..n).map(|i| !gamma_excluded[i] && !pinned[i]).collect();
        let pool = pool_channels(&own_gamma, &candidates);
        (pool.value, mark_excluded(pool.roles, &gamma_excluded))
    } else {
        (None, vec![PoolRole::Unused; n])
    };

    // Step 6: the cast.
    let cast_excluded: Vec<bool> = frames
        .iter()
        .map(|f| {
            matches!(f.settings.colour.cast, CastEstimate::Fixed { .. })
                || !f.settings.colour.auto_colour
        })
        .collect();
    // Each frame's own fit is independent of the others, so the fits run in
    // parallel; the results are collected in input order and the first
    // error in input order is returned, exactly as a serial loop would.
    let fits: Vec<Result<CastCorrection>> = (0..n)
        .into_par_iter()
        .map(|i| {
            let f = &frames[i];
            Ok(match f.settings.colour.cast {
                CastEstimate::Fixed {
                    slope_r,
                    offset_r,
                    slope_b,
                    offset_b,
                } => CastCorrection {
                    slope_r,
                    offset_r,
                    slope_b,
                    offset_b,
                    cells_used: 0,
                },
                CastEstimate::Auto if cast_excluded[i] || film.is_some() => {
                    CastCorrection::IDENTITY
                }
                CastEstimate::Auto => {
                    let gamma = gamma_fixed[i]
                        .or(pooled_gamma)
                        .unwrap_or_else(|| no_profile_gamma(dmin_in_effect[i], dmax_in_effect[i]));
                    let e = invert(&f.analysis.grid_density, dmin_in_effect[i], gamma)?;
                    fit_cast(&e)
                }
            })
        })
        .collect();
    let casts: Vec<CastCorrection> = fits.into_iter().collect::<Result<_>>()?;
    let (pooled_cast, cast_roles) = if !roll.pool_colour {
        (None, vec![PoolRole::Unused; n])
    } else if film.is_some() || !border_pool {
        (
            None,
            mark_excluded(vec![PoolRole::Unused; n], &cast_excluded),
        )
    } else {
        let rejected: Vec<bool> = (0..n)
            .map(|i| dmin_roles[i] == PoolRole::Rejected || gamma_roles[i] == PoolRole::Rejected)
            .collect();
        let candidates: Vec<bool> = (0..n)
            .map(|i| !cast_excluded[i] && !rejected[i] && !pinned[i])
            .collect();
        let (value, mut roles) = pool_cast(&casts, &candidates);
        for i in 0..n {
            if cast_excluded[i] {
                roles[i] = PoolRole::Excluded;
            } else if pinned[i] {
                roles[i] = PoolRole::Unused;
            } else if rejected[i] {
                roles[i] = PoolRole::Rejected;
            }
        }
        (value, roles)
    };

    let records = (0..n)
        .map(|i| PoolFrame {
            source: frames[i].source.to_owned(),
            dmin: own_dmin[i],
            dmin_source: if dmin_excluded[i] {
                DminSource::Fixed
            } else {
                frames[i].analysis.dmin.source
            },
            dmax: dmax_in_effect[i],
            gamma: own_gamma[i],
            cast: casts[i],
            dmin_role: dmin_roles[i],
            gamma_role: gamma_roles[i],
            cast_role: cast_roles[i],
            needs_analysis_area: pinned[i],
        })
        .collect();
    Ok(RollPool {
        dmin: pooled_dmin,
        gamma: pooled_gamma,
        cast: pooled_cast,
        frames: records,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{RenderContext, render_frame, render_frame_with};
    use negg_types::CastSource;

    const W: usize = 128;
    const H: usize = 96;

    /// A synthetic negative as in the pipeline tests: an unexposed border
    /// of `dmin` around a textured ramp from `dmin` to `dmin + gamma`, with
    /// a tint in the interior so the cast fit has cells to use.
    fn negative(dmin: [f32; 3], gamma: [f32; 3]) -> Image {
        negative_with_border(dmin, gamma, 16)
    }

    /// As [`negative`] with a border `border` pixels wide; with 0 the frame
    /// has no unexposed border and its own Dmin is a percentile estimate.
    fn negative_with_border(dmin: [f32; 3], gamma: [f32; 3], border: usize) -> Image {
        let mut img = Image::new(W, H);
        for y in 0..H {
            for x in 0..W {
                let interior = x >= border && x < W - border && y >= border && y < H - border;
                let t = if interior {
                    let ramp = (x - border) as f32 / (W - 2 * border) as f32;
                    let texture = if (x / 3 + y / 3) % 2 == 0 { 0.0 } else { 0.08 };
                    (ramp * 0.9 + texture).min(1.0)
                } else {
                    0.0
                };
                let mut px = [0.0f32; 3];
                for c in 0..3 {
                    px[c] = 10f32.powf(-(dmin[c] + gamma[c] * t));
                }
                img.set_pixel(x, y, px);
            }
        }
        img
    }

    fn analyses(dmins: &[[f32; 3]]) -> Vec<FrameAnalysis> {
        dmins
            .iter()
            .map(|&d| {
                analyse_frame(&negative(d, [1.6, 1.7, 1.8]), None, &Geometry::default()).unwrap()
            })
            .collect()
    }

    fn frames<'a>(
        names: &'a [String],
        analyses: &'a [FrameAnalysis],
        settings: &'a [Settings],
    ) -> Vec<RollFrame<'a>> {
        (0..analyses.len())
            .map(|i| RollFrame {
                source: &names[i],
                analysis: &analyses[i],
                settings: &settings[i],
            })
            .collect()
    }

    fn names(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("f{i}.dng")).collect()
    }

    #[test]
    fn analyse_frame_matches_the_pipeline_estimates() {
        let img = negative([0.2, 0.5, 0.7], [1.6, 1.7, 1.8]);
        let a = analyse_frame(&img, None, &Geometry::default()).unwrap();
        let r = render_frame(&img, &Settings::default(), None).unwrap();
        assert_eq!(a.dmin.value, r.estimates.dmin);
        assert_eq!(a.dmin.source, r.estimates.dmin_source);
        assert_eq!(a.dmax, r.estimates.dmax);
        // With a rig density model the analysis uses the model density.
        let model = DensityModel {
            a: [0.1, 0.1, 0.1],
            g: [[-1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, -1.0]],
        };
        let m = analyse_frame(&img, Some(&model), &Geometry::default()).unwrap();
        for c in 0..3 {
            assert!((m.dmin.value[c] - a.dmin.value[c] - 0.1).abs() < 1e-4);
            assert!((m.dmax[c] - a.dmax[c] - 0.1).abs() < 1e-4);
        }
    }

    #[test]
    fn an_outlier_dmin_frame_is_rejected_and_the_rest_pooled() {
        let dmins = [
            [0.20, 0.50, 0.70],
            [0.21, 0.51, 0.71],
            [0.22, 0.52, 0.72],
            [0.70, 1.00, 1.20],
        ];
        let a = analyses(&dmins);
        let s = vec![Settings::default(); 4];
        let nm = names(4);
        let pool = pool_roll(&frames(&nm, &a, &s), &RollSettings::default(), None).unwrap();
        let roles: Vec<PoolRole> = pool.frames.iter().map(|f| f.dmin_role).collect();
        assert_eq!(
            roles,
            [
                PoolRole::Inlier,
                PoolRole::Inlier,
                PoolRole::Inlier,
                PoolRole::Rejected
            ]
        );
        let pooled = pool.dmin.unwrap();
        // The median of the three inliers is the middle frame's own value.
        assert_eq!(pooled, a[1].dmin.value);
        assert_eq!(pool.frames[3].cast_role, PoolRole::Rejected);
        assert_eq!(pool.frames[0].source, "f0.dng");
        // Gamma is pooled from Dmax minus the pooled Dmin.
        let g = pool.gamma.unwrap();
        for c in 0..3 {
            let want = a[1].dmax[c] - pooled[c];
            assert!((pool.frames[1].gamma[c] - want).abs() < 1e-6);
            assert!(g[c] > 1.0, "{g:?}");
        }
        assert!(pool.cast.is_some());
        assert!(pool.cast.unwrap().cells_used > 0);
    }

    #[test]
    fn fixed_frames_are_excluded_and_do_not_move_the_pool() {
        let dmins = [[0.20, 0.50, 0.70], [0.22, 0.52, 0.72], [0.24, 0.54, 0.74]];
        let a = analyses(&dmins);
        let mut s = vec![Settings::default(); 3];
        s[2].film.film_base = ChannelEstimate::Fixed {
            value: [0.9, 0.9, 0.9],
        };
        s[2].film.gamma = ChannelEstimate::Fixed { value: [2.0; 3] };
        s[1].colour.cast = CastEstimate::Fixed {
            slope_r: 1.0,
            offset_r: 0.01,
            slope_b: 1.0,
            offset_b: -0.01,
        };
        let nm = names(3);
        let pool = pool_roll(&frames(&nm, &a, &s), &RollSettings::default(), None).unwrap();
        assert_eq!(pool.frames[2].dmin_role, PoolRole::Excluded);
        assert_eq!(pool.frames[2].gamma_role, PoolRole::Excluded);
        assert_eq!(pool.frames[2].dmin, [0.9; 3]);
        assert_eq!(pool.frames[2].dmin_source, DminSource::Fixed);
        assert_eq!(pool.frames[2].gamma, [2.0; 3]);
        assert_eq!(pool.frames[1].cast_role, PoolRole::Excluded);
        assert_eq!(pool.frames[1].cast.offset_r, 0.01);
        // Two inliers: the pooled Dmin is their per-channel mean (median of
        // an even count).
        let d = pool.dmin.unwrap();
        for (c, got) in d.iter().enumerate() {
            let want = (a[0].dmin.value[c] + a[1].dmin.value[c]) / 2.0;
            assert!((got - want).abs() < 1e-6);
        }
    }

    #[test]
    fn pool_flags_off_give_no_pool_and_unused_roles() {
        let a = analyses(&[[0.2, 0.5, 0.7], [0.21, 0.51, 0.71]]);
        let s = vec![Settings::default(); 2];
        let nm = names(2);
        let roll = RollSettings {
            pool_film_base: false,
            pool_gamma: false,
            pool_colour: false,
            ..RollSettings::default()
        };
        let pool = pool_roll(&frames(&nm, &a, &s), &roll, None).unwrap();
        assert_eq!(pool.dmin, None);
        assert_eq!(pool.gamma, None);
        assert_eq!(pool.cast, None);
        for f in &pool.frames {
            assert_eq!(
                (f.dmin_role, f.gamma_role, f.cast_role),
                (PoolRole::Unused, PoolRole::Unused, PoolRole::Unused)
            );
        }
        // Each frame's gamma is its own range.
        assert_eq!(
            pool.frames[0].gamma,
            no_profile_gamma(a[0].dmin.value, a[0].dmax)
        );
    }

    #[test]
    fn a_film_correction_pools_no_cast() {
        let a = analyses(&[[0.2, 0.5, 0.7], [0.21, 0.51, 0.71]]);
        let mut s = vec![Settings::default(); 2];
        s[0].colour.auto_colour = false;
        let nm = names(2);
        let film = FilmCorrection {
            name: "stock".into(),
            rig: None,
            gamma_ratio: [0.9, 1.0, 1.1],
            offset_log_d: [0.01, 0.0, -0.02],
        };
        let pool = pool_roll(&frames(&nm, &a, &s), &RollSettings::default(), Some(&film)).unwrap();
        assert_eq!(pool.cast, None);
        assert_eq!(pool.frames[0].cast_role, PoolRole::Excluded);
        assert_eq!(pool.frames[1].cast_role, PoolRole::Unused);
        assert_eq!(pool.frames[1].cast, CastCorrection::IDENTITY);
        assert!(pool.dmin.is_some() && pool.gamma.is_some());
    }

    #[test]
    fn invalid_settings_are_an_error_and_an_empty_roll_pools_nothing() {
        let a = analyses(&[[0.2, 0.5, 0.7]]);
        let mut s = vec![Settings::default()];
        s[0].print.grade = 1e9;
        let nm = names(1);
        assert!(pool_roll(&frames(&nm, &a, &s), &RollSettings::default(), None).is_err());
        let empty = pool_roll(&[], &RollSettings::default(), None).unwrap();
        assert_eq!(empty, RollPool::default());
    }

    #[test]
    fn rendering_with_the_pool_uses_the_pooled_values() {
        let dmins = [[0.20, 0.50, 0.70], [0.21, 0.51, 0.71], [0.22, 0.52, 0.72]];
        let imgs: Vec<Image> = dmins
            .iter()
            .map(|&d| negative(d, [1.6, 1.7, 1.8]))
            .collect();
        let a: Vec<FrameAnalysis> = imgs
            .iter()
            .map(|i| analyse_frame(i, None, &Geometry::default()).unwrap())
            .collect();
        let s = vec![Settings::default(); 3];
        let nm = names(3);
        let roll = RollSettings::default();
        let pool = pool_roll(&frames(&nm, &a, &s), &roll, None).unwrap();
        let pooled = pool.pooled(&roll);
        let ctx = RenderContext {
            pooled: Some(&pooled),
            ..RenderContext::default()
        };
        for img in &imgs {
            let r = render_frame_with(img, &Settings::default(), &ctx, None).unwrap();
            assert_eq!(r.estimates.dmin, pool.dmin.unwrap());
            assert_eq!(r.estimates.dmin_source, DminSource::Roll);
            assert_eq!(r.estimates.gamma, pool.gamma.unwrap());
            assert_eq!(r.estimates.cast, pool.cast.unwrap());
        }
    }

    /// Pass 1, the pool and pass 2 over `imgs` with default settings: the
    /// pool and each frame's render estimates.
    fn pool_and_render(imgs: &[Image]) -> (RollPool, Vec<negg_types::Estimates>) {
        let a: Vec<FrameAnalysis> = imgs
            .iter()
            .map(|i| analyse_frame(i, None, &Geometry::default()).unwrap())
            .collect();
        let s = vec![Settings::default(); imgs.len()];
        let nm = names(imgs.len());
        let roll = RollSettings::default();
        let pool = pool_roll(&frames(&nm, &a, &s), &roll, None).unwrap();
        let pooled = pool.pooled(&roll);
        let ctx = RenderContext {
            pooled: Some(&pooled),
            ..RenderContext::default()
        };
        let estimates = imgs
            .iter()
            .map(|img| {
                render_frame_with(img, &Settings::default(), &ctx, None)
                    .unwrap()
                    .estimates
            })
            .collect();
        (pool, estimates)
    }

    #[test]
    fn a_percentile_frame_keeps_its_own_base_below_the_pool() {
        let dmins = [
            [0.50, 0.80, 1.00],
            [0.50, 0.80, 1.00],
            [0.50, 0.80, 1.00],
            [0.15, 0.45, 0.65],
        ];
        let imgs: Vec<Image> = dmins
            .iter()
            .map(|&d| negative_with_border(d, [1.6, 1.7, 1.8], 0))
            .collect();
        let (pool, est) = pool_and_render(&imgs);
        let own = &pool.frames[3];
        assert_eq!(own.dmin_source, DminSource::Percentile);
        assert_eq!(own.dmin_role, PoolRole::Rejected);
        let pooled = pool.dmin.unwrap();
        // Ruling 105: d renders with its own base, not the higher pool.
        assert_eq!(est[3].dmin, own.dmin);
        assert_eq!(est[3].dmin_source, DminSource::Percentile);
        assert!((0..3).all(|c| own.dmin[c] < pooled[c]));
        // The others' own estimates equal the pool, which they take.
        for e in &est[..3] {
            assert_eq!((e.dmin, e.dmin_source), (pooled, DminSource::Roll));
        }
        // gamma_f uses the Dmin the frame renders with.
        for c in 0..3 {
            let want = (own.dmax[c] - own.dmin[c]).max(0.05);
            assert!((own.gamma[c] - want).abs() < 1e-6, "{:?}", own.gamma);
        }
        // Ruling 106: a percentile pool gives no pooled cast; each frame is
        // corrected with its own fit, the one the pool recorded.
        assert_eq!(pool.cast, None);
        for (f, e) in pool.frames.iter().zip(&est) {
            assert_eq!(f.cast_role, PoolRole::Unused);
            assert_eq!(e.cast_source, CastSource::Fit);
            assert_eq!(e.cast, f.cast);
        }
    }

    #[test]
    fn a_border_frame_rejected_from_the_pool_still_takes_it() {
        let dmins = [
            [0.50, 0.80, 1.00],
            [0.50, 0.80, 1.00],
            [0.50, 0.80, 1.00],
            [0.15, 0.45, 0.65],
        ];
        let imgs: Vec<Image> = dmins
            .iter()
            .map(|&d| negative(d, [1.6, 1.7, 1.8]))
            .collect();
        let (pool, est) = pool_and_render(&imgs);
        assert_eq!(pool.frames[3].dmin_source, DminSource::Border);
        assert_eq!(pool.frames[3].dmin_role, PoolRole::Rejected);
        let pooled = pool.dmin.unwrap();
        for e in &est {
            assert_eq!((e.dmin, e.dmin_source), (pooled, DminSource::Roll));
        }
        // A Border pool still pools the cast (Ruling 49).
        let cast = pool.cast.unwrap();
        for e in &est {
            assert_eq!((e.cast, e.cast_source), (cast, CastSource::Roll));
        }
    }

    #[test]
    fn one_border_frame_among_percentile_frames_pools_no_cast() {
        // Fewer than two Border frames: every frame is a Dmin candidate, so
        // the pool did not come from Border frames alone (Ruling 106).
        let imgs = vec![
            negative([0.2, 0.5, 0.7], [1.6, 1.7, 1.8]),
            negative_with_border([0.2, 0.5, 0.7], [1.6, 1.7, 1.8], 0),
            negative_with_border([0.2, 0.5, 0.7], [1.6, 1.7, 1.8], 0),
        ];
        let (pool, est) = pool_and_render(&imgs);
        assert_eq!(pool.frames[0].dmin_source, DminSource::Border);
        assert_eq!(pool.frames[0].dmin_role, PoolRole::Inlier);
        assert!(pool.dmin.is_some());
        assert_eq!(pool.cast, None);
        assert!(est.iter().all(|e| e.cast_source == CastSource::Fit));
    }

    /// `film` inside an opaque surround `margin` pixels wide on every side
    /// (transmittance 0: a film holder in the shot).
    fn in_holder(film: &Image, margin: usize) -> Image {
        let (w, h) = (film.width() + 2 * margin, film.height() + 2 * margin);
        let mut img = Image::new(w, h);
        for y in 0..film.height() {
            for x in 0..film.width() {
                img.set_pixel(x + margin, y + margin, film.pixel(x, y));
            }
        }
        img
    }

    /// The analysis area of a `film_w` by `film_h` film inside a surround
    /// `margin` pixels wide, with no analysis buffer.
    fn film_area(film_w: usize, film_h: usize, margin: usize) -> Geometry {
        let (w, h) = ((film_w + 2 * margin) as f32, (film_h + 2 * margin) as f32);
        Geometry {
            analysis_area: Some(negg_types::CropRect {
                x: margin as f32 / w,
                y: margin as f32 / h,
                width: film_w as f32 / w,
                height: film_h as f32 / h,
            }),
            analysis_buffer: 0.0,
            ..Geometry::default()
        }
    }

    #[test]
    fn an_analysis_area_on_the_film_gives_the_clean_estimates_at_full_size() {
        let clean = negative([0.2, 0.5, 0.7], [1.6, 1.7, 1.8]);
        let held = in_holder(&clean, 24);
        // Without an analysis area the holder pins Dmax at the cap.
        let pinned = render_frame(&held, &Settings::default(), None).unwrap();
        assert!(
            negg_types::dmax_pinned(&pinned.estimates.dmax),
            "{:?}",
            pinned.estimates
        );
        // With the area on the film, the estimates are the clean frame's,
        // and the output keeps the whole frame (no crop).
        let mut s = Settings {
            geometry: film_area(W, H, 24),
            ..Settings::default()
        };
        let r = render_frame(&held, &s, None).unwrap();
        let want = render_frame(&clean, &Settings::default(), None).unwrap();
        assert_eq!(r.estimates, want.estimates);
        assert_eq!(r.grid_cells, want.grid_cells);
        assert_eq!((r.image.width(), r.image.height()), (W + 48, H + 48));
        assert!(!negg_types::dmax_pinned(&r.estimates.dmax));
        // Pass 1 of roll mode sees the same.
        let a = analyse_frame(&held, None, &s.geometry).unwrap();
        let b = analyse_frame(&clean, None, &Geometry::default()).unwrap();
        assert_eq!(a, b);
        // A crop elsewhere does not move the analysis while the area is set.
        s.geometry.crop = Some(negg_types::CropRect {
            x: 0.0,
            y: 0.0,
            width: 0.5,
            height: 0.5,
        });
        let cropped = render_frame(&held, &s, None).unwrap();
        assert_eq!(cropped.estimates, want.estimates);
    }

    #[test]
    fn holder_pinned_frames_are_left_out_of_the_gamma_and_cast_pools() {
        let good = [
            negative([0.20, 0.50, 0.70], [1.60, 1.70, 1.80]),
            negative([0.21, 0.51, 0.71], [1.55, 1.65, 1.80]),
            negative([0.22, 0.52, 0.72], [1.60, 1.70, 1.75]),
        ];
        let held: Vec<Image> = (0..5)
            .map(|i| {
                let d = 0.2 + 0.01 * i as f32;
                in_holder(&negative([d, d + 0.3, d + 0.5], [1.6, 1.7, 1.8]), 20)
            })
            .collect();
        let imgs: Vec<Image> = good.iter().chain(&held).cloned().collect();
        let (pool, est) = pool_and_render(&imgs);
        let (good_pool, _) = pool_and_render(&good);
        // The pool is the three good frames' alone.
        assert!(pool.gamma.is_some());
        assert_eq!(pool.dmin, good_pool.dmin);
        assert_eq!(pool.gamma, good_pool.gamma);
        assert_eq!(pool.cast, good_pool.cast);
        for (i, f) in pool.frames.iter().enumerate() {
            if i < 3 {
                assert!(!f.needs_analysis_area, "{f:?}");
                assert_eq!(f.gamma_role, PoolRole::Inlier, "{f:?}");
                assert_eq!(f.cast_role, PoolRole::Inlier, "{f:?}");
            } else {
                assert!(f.needs_analysis_area, "{f:?}");
                assert!(negg_types::dmax_pinned(&f.dmax));
                assert_eq!(f.gamma_role, PoolRole::Unused, "{f:?}");
                assert_eq!(f.cast_role, PoolRole::Unused, "{f:?}");
            }
        }
        // Every frame, the pinned ones too, renders with the good pool.
        for e in &est {
            assert_eq!(Some(e.gamma), pool.gamma);
            assert_eq!(e.gamma_source, negg_types::GammaSource::Roll);
        }
    }

    #[test]
    fn an_all_pinned_roll_has_no_gamma_pool() {
        let held: Vec<Image> = (0..3)
            .map(|i| {
                let d = 0.2 + 0.01 * i as f32;
                in_holder(&negative([d, d + 0.3, d + 0.5], [1.6, 1.7, 1.8]), 20)
            })
            .collect();
        let (pool, est) = pool_and_render(&held);
        assert_eq!(pool.gamma, None);
        assert_eq!(pool.cast, None);
        for (f, e) in pool.frames.iter().zip(&est) {
            assert!(f.needs_analysis_area);
            assert_eq!(f.gamma_role, PoolRole::Unused);
            assert_eq!(f.cast_role, PoolRole::Unused);
            // Each keeps its own gamma.
            assert_eq!(e.gamma_source, negg_types::GammaSource::Range);
        }
        // A fixed Dmax below the cap takes the frame back into the pool.
        let a: Vec<FrameAnalysis> = held
            .iter()
            .map(|i| analyse_frame(i, None, &Geometry::default()).unwrap())
            .collect();
        let mut s = vec![Settings::default(); 3];
        s[0].film.dmax = ChannelEstimate::Fixed {
            value: [2.0, 2.3, 2.6],
        };
        let nm = names(3);
        let pool = pool_roll(&frames(&nm, &a, &s), &RollSettings::default(), None).unwrap();
        assert!(!pool.frames[0].needs_analysis_area);
        assert_eq!(pool.frames[0].gamma_role, PoolRole::Inlier);
        assert!(pool.gamma.is_some());
        assert!(pool.frames[1].needs_analysis_area && pool.frames[2].needs_analysis_area);
    }
}
