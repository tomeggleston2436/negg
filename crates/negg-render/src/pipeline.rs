// SPDX-License-Identifier: MPL-2.0

//! Steps 3 to 7 of `docs/MATHS.md` orchestrated for one frame.
//!
//! [`render_frame_with`] is the one pipeline of ground rule 3: it builds
//! the analysis grid and its density (step 3, with the rig density model
//! when the [`RenderContext`] carries one), runs [`estimate_frame`] (Dmin,
//! Dmax and gamma by the R-lock precedence, the film correction's neutral
//! offset, the cast by R-cast-source, the exposure anchor and `x_0`), then
//! renders the full-resolution frame in one fused pass with
//! [`render_full`] (steps 3 to 7: the density is taken per pixel inside the
//! kernel). [`render_frame`] is the same with the default context, which
//! renders exactly as phase 1. The estimators themselves live in
//! `negg-film`; this module only calls them in order. It does no I/O and
//! no colour management: the camera matrix of step 7 is computed by the
//! caller with `negg-cms` and passed in. The linear part of step 2 (flat
//! field, unmix) is applied by the caller beforehand with
//! [`crate::apply_rig_linear`] (R-rig-order).
//!
//! Memory: besides the input, one full-size `f32` image is allocated, the
//! output of [`render_full`]; no full-size density image is made. The
//! grid-sized intermediates are about a thousand
//! pixels on the long side and are not counted.

use negg_film::{
    AnalysisGrid, DminEstimate, analysis_grid, estimate_dmax, estimate_dmin, exposure_anchor,
    no_profile_gamma,
};
use negg_types::settings::ChannelEstimate;
use negg_types::{
    CastCorrection, CastEstimate, CastSource, DensityModel, DminSource, Error, Estimates,
    FilmCorrection, GammaSource, Image, PooledEstimates, Result, Settings,
};

use crate::cast::{apply_cast, fit_cast};
use crate::full::render_full;
use crate::invert::invert;
use crate::print::{PrintCoefficients, solve_x0};
use crate::rig::density_grid;

/// Long side of the analysis grid of step 3 of `docs/MATHS.md`, in cells
/// ("about 1024 px").
pub const ANALYSIS_GRID_LONG_SIDE: usize = 1024;

/// The result of [`render_frame`]: the rendered positive plus everything
/// that was estimated to produce it.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameRender {
    /// The positive after step 5 and, when a matrix was given, step 7:
    /// linear Rec.2020 (or whatever working space the matrix maps into)
    /// when `camera_to_working` was `Some`, otherwise linear camera RGB.
    pub image: Image,
    /// Steps 3 to 6: what was estimated or fixed for this frame, as written
    /// to the sidecar (step 8).
    pub estimates: Estimates,
    /// Number of cells in the analysis grid of step 3.
    pub grid_cells: u32,
}

/// What a frame is rendered with besides its own pixels and settings
/// (steps 2 to 6 of `docs/MATHS.md`, phase 2): the rig density model, the
/// film correction and the roll's pooled estimates. Every part is
/// optional; the default context renders exactly as phase 1.
#[derive(Clone, Copy, Debug, Default)]
pub struct RenderContext<'a> {
    /// The rig density model replacing the step 3 density
    /// (R-density-use).
    pub density: Option<&'a DensityModel>,
    /// The film correction: gamma ratios and neutral offsets (R-contrast).
    pub film: Option<&'a FilmCorrection>,
    /// The roll's pooled Dmin, gamma and cast, already filtered by the
    /// roll's `pool_*` flags (R-pool, R-cast-pool).
    pub pooled: Option<&'a PooledEstimates>,
}

/// Step 3 of `docs/MATHS.md`: Dmin and Dmax estimated on `grid_density`.
///
/// Without a density model the estimators run on the grid itself, exactly
/// as phase 1 (their density is the default formula). With one, they run on
/// a grid whose block medians are `10^-D` of the model density and whose
/// `cv` is the original grid's, so the default formula reads the model
/// density back and the border test keeps its texture measure. The round
/// trip costs about one ulp; a density above 6 (transmittance below the
/// `1e-6` floor) reads back as 6.
pub(crate) fn estimate_dmin_dmax(
    grid: &AnalysisGrid,
    grid_density: &Image,
    model: Option<&DensityModel>,
) -> (DminEstimate, [f32; 3]) {
    match model {
        None => (estimate_dmin(grid), estimate_dmax(grid)),
        Some(_) => {
            let data = grid_density.data().iter().map(|d| 10f32.powf(-d)).collect();
            let median = Image::from_vec(grid_density.width(), grid_density.height(), data)
                .expect("same shape as the density grid");
            let as_linear = AnalysisGrid {
                median,
                cv: grid.cv.clone(),
                block: grid.block,
            };
            (estimate_dmin(&as_linear), estimate_dmax(&as_linear))
        }
    }
}

/// Checks a film correction's numbers before they reach step 4.
fn check_film(film: &FilmCorrection) -> Result<()> {
    let ok_ratio = film.gamma_ratio.iter().all(|r| r.is_finite() && *r > 0.0);
    let ok_offset = film.offset_log_d.iter().all(|o| o.is_finite());
    if ok_ratio && ok_offset {
        Ok(())
    } else {
        Err(Error::Invalid(format!(
            "film correction {}: gamma ratios {:?} must be finite and positive and \
             offsets {:?} finite",
            film.name, film.gamma_ratio, film.offset_log_d
        )))
    }
}

/// Steps 3 to 6 of `docs/MATHS.md` for one frame on its analysis grid:
/// every estimate the render needs, by the precedence of R-lock (ADR 0017),
/// R-contrast (ADR 0016) and R-cast-source (ADR 0018).
///
/// `grid_density` must be the density of `grid.median` as step 3 defines
/// it for this context, that is `density_grid(&grid.median, ctx.density)`.
///
/// - Dmin: settings Fixed (`Fixed`) > `ctx.pooled` Dmin (`Roll`) > the
///   frame's estimate (`Border` or `Percentile`).
/// - Dmax: settings Fixed > the frame's estimate.
/// - Gamma: settings Fixed (`Fixed`) > the film correction (`Profile`,
///   `gamma_c = ratio_c * gamma_G` with `gamma_G` the pooled green gamma
///   when there is one, else the green range `max(Dmax_G - Dmin_G, 0.05)`)
///   > `ctx.pooled` gamma (`Roll`) > the range (`Range`).
/// - `neutral_offset`: with a film correction, `offset_log_d / gamma_G`
///   with `gamma_G` the green entry of the gamma in effect; else zero. It is
///   subtracted from `e` after the inversion, before step 6.
/// - Cast: `colour.cast` Fixed (`Fixed`, even with `auto_colour` off) >
///   the identity when `auto_colour` is off or a film correction is in use
///   (`Identity`) > `ctx.pooled` cast (`Roll`) > [`fit_cast`] on the
///   inverted grid (`Fit`, or `Identity` when no cells survive).
/// - The anchor and `x_0` as phase 1: [`exposure_anchor`] on the inverted,
///   offset and cast-corrected grid, then [`solve_x0`].
///
/// # Errors
///
/// [`Error::Invalid`] when the settings fail validation, the grid has no
/// cells, `grid_density` does not match the grid, the film correction has
/// a non-finite or non-positive ratio or a non-finite offset, the gamma in
/// effect is not finite and positive, or the exposure anchor cannot be
/// printed at `P = 0.75` with the paper densities in `settings.print`.
pub fn estimate_frame(
    grid: &AnalysisGrid,
    grid_density: &Image,
    settings: &Settings,
    ctx: &RenderContext,
) -> Result<Estimates> {
    settings.validate()?;
    if grid.cells() == 0 {
        return Err(Error::Invalid(
            "analysis grid has no cells: nothing to estimate".to_string(),
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
    if let Some(film) = ctx.film {
        check_film(film)?;
    }
    let pooled = ctx.pooled.copied().unwrap_or_default();

    // Step 3: Dmin and Dmax. The estimators run only when a value is needed.
    let needs_estimate = matches!(settings.film.film_base, ChannelEstimate::Auto)
        || matches!(settings.film.dmax, ChannelEstimate::Auto);
    let estimated = needs_estimate.then(|| estimate_dmin_dmax(grid, grid_density, ctx.density));
    let (dmin, dmin_source) = match (settings.film.film_base, pooled.dmin, estimated) {
        (ChannelEstimate::Fixed { value }, _, _) => (value, DminSource::Fixed),
        (ChannelEstimate::Auto, Some(value), _) => (value, DminSource::Roll),
        (ChannelEstimate::Auto, None, Some((est, _))) => (est.value, est.source),
        (ChannelEstimate::Auto, None, None) => unreachable!("estimated when Auto"),
    };
    let dmax = match (settings.film.dmax, estimated) {
        (ChannelEstimate::Fixed { value }, _) => value,
        (ChannelEstimate::Auto, Some((_, dmax))) => dmax,
        (ChannelEstimate::Auto, None) => unreachable!("estimated when Auto"),
    };

    // Step 4: gamma by precedence, then the film correction's offsets.
    let range = no_profile_gamma(dmin, dmax);
    let (gamma, gamma_source) = match (settings.film.gamma, ctx.film, pooled.gamma) {
        (ChannelEstimate::Fixed { value }, _, _) => (value, GammaSource::Fixed),
        (ChannelEstimate::Auto, Some(film), pooled_gamma) => {
            let gamma_g = pooled_gamma.map_or(range[1], |g| g[1]);
            let r = film.gamma_ratio;
            (
                [r[0] * gamma_g, r[1] * gamma_g, r[2] * gamma_g],
                GammaSource::Profile,
            )
        }
        (ChannelEstimate::Auto, None, Some(value)) => (value, GammaSource::Roll),
        (ChannelEstimate::Auto, None, None) => (range, GammaSource::Range),
    };
    let neutral_offset = match ctx.film {
        Some(film) => {
            let o = film.offset_log_d;
            [o[0] / gamma[1], o[1] / gamma[1], o[2] / gamma[1]]
        }
        None => [0.0; 3],
    };

    // Step 4 on the grid (validates gamma), then the offsets.
    let mut e_grid = invert(grid_density, dmin, gamma)?;
    if ctx.film.is_some() {
        for px in e_grid.data_mut().as_chunks_mut::<3>().0 {
            for c in 0..3 {
                px[c] -= neutral_offset[c];
            }
        }
    }

    // Step 6: the cast by precedence, applied on the grid so the anchor is
    // taken from the corrected exposure.
    let (cast, cast_source) = match settings.colour.cast {
        CastEstimate::Fixed {
            slope_r,
            offset_r,
            slope_b,
            offset_b,
        } => (
            CastCorrection {
                slope_r,
                offset_r,
                slope_b,
                offset_b,
                cells_used: 0,
            },
            CastSource::Fixed,
        ),
        CastEstimate::Auto if !settings.colour.auto_colour || ctx.film.is_some() => {
            (CastCorrection::IDENTITY, CastSource::Identity)
        }
        CastEstimate::Auto => match pooled.cast {
            Some(cast) => (cast, CastSource::Roll),
            None => {
                let cast = fit_cast(&e_grid);
                let source = if cast.cells_used > 0 {
                    CastSource::Fit
                } else {
                    CastSource::Identity
                };
                (cast, source)
            }
        },
    };
    apply_cast(&mut e_grid, &cast);

    // Step 5: the exposure anchor and x_0.
    let anchor = exposure_anchor(grid, &e_grid);
    let x0 = solve_x0(anchor, &settings.print);
    if !x0.is_finite() {
        return Err(Error::Invalid(format!(
            "exposure anchor {anchor} cannot be printed at P = 0.75 with paper_dmin {} \
             and paper_dmax {}: x0 is not finite",
            settings.print.paper_dmin, settings.print.paper_dmax
        )));
    }
    Ok(Estimates {
        dmin,
        dmin_source,
        dmax,
        gamma,
        gamma_source,
        neutral_offset,
        anchor,
        x0,
        cast,
        cast_source,
    })
}

/// Steps 3 to 7 of `docs/MATHS.md` for one frame with a phase 2 context:
/// analyse `linear` (linear camera RGB after the caller's step 2
/// [`crate::apply_rig_linear`], or linear Rec.2020 for an input that was
/// linearised through its own ICC profile) with `settings` and `ctx`, and
/// render the positive.
///
/// In order, each naming its step:
///
/// 1. `settings.validate()`.
/// 2. Step 3: the analysis grid with [`analysis_grid`] at
///    [`ANALYSIS_GRID_LONG_SIDE`], and its density with [`density_grid`]
///    (the rig model `ctx.density` when given, R-density-use).
/// 3. Steps 3 to 6: [`estimate_frame`] (Dmin, Dmax, gamma, neutral offset,
///    cast, anchor and `x_0` by the R-lock precedence).
/// 4. Step 5: [`PrintCoefficients::new`].
/// 5. Steps 3 to 7 fused on the full frame: [`render_full`] on `linear`
///    with `ctx.density` (the density of step 3 taken per pixel) and
///    `camera_to_working`, the camera-to-working-space matrix built by the
///    caller with `negg-cms` (`None` skips step 7, leaving the output in
///    camera RGB; a rig does not change it, R-step7).
///
/// # Errors
///
/// [`Error::Invalid`] when `linear` has no pixels, and every error of
/// [`estimate_frame`] (invalid settings, a fixed gamma that is not
/// positive, a bad film correction, an anchor that cannot be printed at
/// `P = 0.75`; `x_0` is never stored when it is not finite).
pub fn render_frame_with(
    linear: &Image,
    settings: &Settings,
    ctx: &RenderContext,
    camera_to_working: Option<[[f32; 3]; 3]>,
) -> Result<FrameRender> {
    settings.validate()?;
    if linear.width() == 0 || linear.height() == 0 {
        return Err(Error::Invalid(format!(
            "image is {}x{}: nothing to render",
            linear.width(),
            linear.height()
        )));
    }

    // Step 3: the block-median analysis grid and its density.
    let grid = analysis_grid(linear, ANALYSIS_GRID_LONG_SIDE);
    let grid_cells = u32::try_from(grid.cells()).unwrap_or(u32::MAX);
    let grid_density = density_grid(&grid.median, ctx.density);

    // Steps 3 to 6 on the grid.
    let estimates = estimate_frame(&grid, &grid_density, settings, ctx)?;
    let coefficients = PrintCoefficients::new(&settings.print, estimates.x0)?;
    drop(grid_density);
    drop(grid);

    // Steps 3 to 7 fused on the full frame: the density is taken per pixel
    // inside the kernel, so the output is the only full-size allocation.
    let image = render_full(
        linear,
        &estimates,
        &coefficients,
        ctx.density,
        camera_to_working,
    )?;

    Ok(FrameRender {
        image,
        estimates,
        grid_cells,
    })
}

/// Steps 3 to 7 of `docs/MATHS.md` for one frame, as phase 1: no rig
/// density model, no film correction and no roll pool. Exactly
/// [`render_frame_with`] with [`RenderContext::default`]; see there for
/// the steps.
///
/// # Errors
///
/// As [`render_frame_with`].
pub fn render_frame(
    linear: &Image,
    settings: &Settings,
    camera_to_working: Option<[[f32; 3]; 3]>,
) -> Result<FrameRender> {
    render_frame_with(
        linear,
        settings,
        &RenderContext::default(),
        camera_to_working,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::print::ANCHOR_PRINT_DENSITY;
    use negg_types::settings::PrintSettings;

    const W: usize = 256;
    const H: usize = 192;

    /// A synthetic negative: an unexposed border of `dmin` density around
    /// an interior whose density ramps from `dmin` to `dmin + gamma`, with
    /// a checker texture so the interior is not flat. Transmittance is
    /// `10^-D` per channel.
    fn synthetic_negative(dmin: [f32; 3], gamma: [f32; 3]) -> Image {
        let mut img = Image::new(W, H);
        let border = 24;
        for y in 0..H {
            for x in 0..W {
                let interior = x >= border && x < W - border && y >= border && y < H - border;
                let t = if interior {
                    let ramp = (x - border) as f32 / (W - 2 * border) as f32;
                    let texture = if (x / 4 + y / 4) % 2 == 0 { 0.0 } else { 0.08 };
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

    #[test]
    fn synthetic_negative_renders_with_estimated_sources() {
        let dmin = [0.2, 0.5, 0.7];
        let gamma = [1.7, 1.7, 1.6];
        let img = synthetic_negative(dmin, gamma);
        let r = render_frame(&img, &Settings::default(), None).unwrap();
        assert_eq!(r.image.width(), W);
        assert_eq!(r.image.height(), H);
        assert_eq!(r.grid_cells, (W * H) as u32);
        assert!(
            matches!(
                r.estimates.dmin_source,
                DminSource::Border | DminSource::Percentile
            ),
            "{:?}",
            r.estimates
        );
        assert_eq!(r.estimates.gamma_source, GammaSource::Range);
        for c in 0..3 {
            assert!(
                (r.estimates.dmin[c] - dmin[c]).abs() < 0.01,
                "dmin {:?}",
                r.estimates.dmin
            );
            // Dmax is the 99.5th percentile of a ramp that tops out at
            // dmin + gamma * 0.98.
            assert!(
                (r.estimates.dmax[c] - (dmin[c] + gamma[c] * 0.98)).abs() < 0.1,
                "dmax {:?}",
                r.estimates.dmax
            );
            assert!(r.estimates.gamma[c] > 0.0);
        }
        assert!(r.estimates.anchor <= 0.0 && r.estimates.anchor >= -1.0);
        assert!(r.estimates.x0.is_finite());
        for v in r.image.data() {
            assert!(*v > 0.0 && *v <= 1.0, "{v}");
        }
    }

    #[test]
    fn neutral_negative_renders_neutral_and_anchors_at_0_75() {
        let img = synthetic_negative([0.3; 3], [1.6; 3]);
        let r = render_frame(&img, &Settings::default(), None).unwrap();
        for px in r.image.data().as_chunks::<3>().0 {
            assert!(
                (px[0] - px[1]).abs() < 1e-3 && (px[1] - px[2]).abs() < 1e-3,
                "{px:?}"
            );
        }
        // A pixel whose e equals the anchor prints at P = 0.75.
        let est = r.estimates;
        let e = est.anchor;
        let d = est.dmin[1] - e * est.gamma[1];
        let mut one = Image::new(1, 1);
        one.set_pixel(0, 0, [10f32.powf(-d); 3]);
        let c = PrintCoefficients::new(&PrintSettings::default(), est.x0).unwrap();
        let out = render_full(&one, &est, &c, None, None).unwrap();
        let want = 10f32.powf(-ANCHOR_PRINT_DENSITY);
        assert!(
            ((out.pixel(0, 0)[1] - want) / want).abs() < 1e-3,
            "{:?} want {want}",
            out.pixel(0, 0)
        );
        // A denser negative (brighter scene) prints brighter: the border
        // (least dense) is the darkest part of the positive.
        let border = r.image.pixel(2, 2)[1];
        let bright = r.image.pixel(W - 30, H / 2)[1];
        assert!(bright > border, "border {border} bright {bright}");
    }

    #[test]
    fn fixed_overrides_are_honoured_and_reported() {
        let img = synthetic_negative([0.2, 0.5, 0.7], [1.7, 1.7, 1.6]);
        let mut settings = Settings::default();
        settings.film.film_base = ChannelEstimate::Fixed {
            value: [0.25, 0.55, 0.75],
        };
        settings.film.dmax = ChannelEstimate::Fixed {
            value: [2.0, 2.3, 2.4],
        };
        settings.film.gamma = ChannelEstimate::Fixed {
            value: [1.5, 1.5, 1.5],
        };
        let r = render_frame(&img, &settings, None).unwrap();
        assert_eq!(r.estimates.dmin, [0.25, 0.55, 0.75]);
        assert_eq!(r.estimates.dmin_source, DminSource::Fixed);
        assert_eq!(r.estimates.dmax, [2.0, 2.3, 2.4]);
        assert_eq!(r.estimates.gamma, [1.5; 3]);
        assert_eq!(r.estimates.gamma_source, GammaSource::Fixed);

        // A fixed gamma that cannot be inverted through is rejected.
        settings.film.gamma = ChannelEstimate::Fixed {
            value: [1.5, 0.0, 1.5],
        };
        assert!(matches!(
            render_frame(&img, &settings, None),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn auto_colour_off_gives_identity_cast() {
        let img = synthetic_negative([0.2, 0.5, 0.7], [1.7, 1.7, 1.6]);
        let mut settings = Settings::default();
        settings.colour.auto_colour = false;
        let r = render_frame(&img, &settings, None).unwrap();
        assert_eq!(r.estimates.cast, CastCorrection::IDENTITY);
        let on = render_frame(&img, &Settings::default(), None).unwrap();
        assert!(on.estimates.cast.cells_used > 0, "{:?}", on.estimates.cast);
    }

    #[test]
    fn matrix_is_applied_after_the_print_stage() {
        let img = synthetic_negative([0.3; 3], [1.6; 3]);
        let without = render_frame(&img, &Settings::default(), None).unwrap();
        let m = [[0.5, 0.25, 0.25], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let with = render_frame(&img, &Settings::default(), Some(m)).unwrap();
        assert_eq!(with.estimates, without.estimates);
        for (a, b) in with
            .image
            .data()
            .as_chunks::<3>()
            .0
            .iter()
            .zip(without.image.data().as_chunks::<3>().0)
        {
            assert_eq!(*a, crate::apply_matrix_pixel(*b, m));
        }
    }

    /// The phase 1 `render_frame` body, verbatim apart from the
    /// `render_full` call gaining its `None` density argument: the default
    /// context must reproduce it bit for bit.
    fn phase1_render_frame(
        linear: &Image,
        settings: &Settings,
        camera_to_working: Option<[[f32; 3]; 3]>,
    ) -> Result<FrameRender> {
        use negg_film::density;
        settings.validate()?;
        let grid = analysis_grid(linear, ANALYSIS_GRID_LONG_SIDE);
        let grid_cells = u32::try_from(grid.cells()).unwrap_or(u32::MAX);
        let grid_density = density(&grid.median);
        let (dmin, dmin_source) = match settings.film.film_base {
            ChannelEstimate::Auto => {
                let est = estimate_dmin(&grid);
                (est.value, est.source)
            }
            ChannelEstimate::Fixed { value } => (value, DminSource::Fixed),
        };
        let dmax = match settings.film.dmax {
            ChannelEstimate::Auto => estimate_dmax(&grid),
            ChannelEstimate::Fixed { value } => value,
        };
        let (gamma, gamma_source) = match settings.film.gamma {
            ChannelEstimate::Auto => (no_profile_gamma(dmin, dmax), GammaSource::Range),
            ChannelEstimate::Fixed { value } => (value, GammaSource::Fixed),
        };
        let mut e_grid = invert(&grid_density, dmin, gamma)?;
        let cast = if settings.colour.auto_colour {
            fit_cast(&e_grid)
        } else {
            CastCorrection::IDENTITY
        };
        apply_cast(&mut e_grid, &cast);
        let anchor = exposure_anchor(&grid, &e_grid);
        let x0 = solve_x0(anchor, &settings.print);
        let estimates = Estimates {
            dmin,
            dmin_source,
            dmax,
            gamma,
            gamma_source,
            neutral_offset: [0.0; 3],
            anchor,
            x0,
            cast,
            cast_source: if settings.colour.auto_colour && cast.cells_used > 0 {
                CastSource::Fit
            } else {
                CastSource::Identity
            },
        };
        let coefficients = PrintCoefficients::new(&settings.print, x0)?;
        let image = render_full(linear, &estimates, &coefficients, None, camera_to_working)?;
        Ok(FrameRender {
            image,
            estimates,
            grid_cells,
        })
    }

    /// A frame larger than the grid, so blocks are real medians and `cv`
    /// is not zero everywhere.
    fn textured_negative(w: usize, h: usize, dmin: [f32; 3], gamma: [f32; 3]) -> Image {
        let mut img = Image::new(w, h);
        let border = w / 10;
        for y in 0..h {
            for x in 0..w {
                let interior = x >= border && x < w - border && y >= border && y < h - border;
                let t = if interior {
                    let ramp = (x - border) as f32 / (w - 2 * border) as f32;
                    let texture = ((x * 7 + y * 13) % 17) as f32 / 17.0 * 0.1;
                    (ramp * 0.85 + texture).min(1.0)
                } else {
                    0.0
                };
                let mut px = [0.0f32; 3];
                for c in 0..3 {
                    let tint = if c == 0 { 0.03 * t * t } else { 0.0 };
                    px[c] = 10f32.powf(-(dmin[c] + gamma[c] * t + tint));
                }
                img.set_pixel(x, y, px);
            }
        }
        img
    }

    #[test]
    fn default_context_renders_bit_identical_to_phase_1() {
        let m = [[0.9, 0.1, 0.0], [0.05, 0.9, 0.05], [0.0, 0.2, 0.8]];
        let mut fixed = Settings::default();
        fixed.film.gamma = ChannelEstimate::Fixed {
            value: [1.5, 1.6, 1.7],
        };
        let mut off = Settings::default();
        off.colour.auto_colour = false;
        let small = synthetic_negative([0.2, 0.5, 0.7], [1.7, 1.7, 1.6]);
        let large = textured_negative(1100, 700, [0.25, 0.55, 0.8], [1.6, 1.8, 2.0]);
        // Every settings variant on the small frame; the default on the
        // large one (real block medians, non-zero cv) to keep debug runs
        // short.
        let default = Settings::default();
        let cases: [(&Image, &[&Settings]); 2] =
            [(&small, &[&default, &fixed, &off]), (&large, &[&default])];
        for (img, all_settings) in cases {
            for &settings in all_settings {
                for matrix in [None, Some(m)] {
                    let want = phase1_render_frame(img, settings, matrix).unwrap();
                    let got = render_frame(img, settings, matrix).unwrap();
                    let with = render_frame_with(img, settings, &RenderContext::default(), matrix)
                        .unwrap();
                    assert_eq!(got.estimates, want.estimates);
                    assert_eq!(got.grid_cells, want.grid_cells);
                    let bits = |r: &FrameRender| -> Vec<u32> {
                        r.image.data().iter().map(|v| v.to_bits()).collect()
                    };
                    assert_eq!(bits(&got), bits(&want));
                    assert_eq!(bits(&with), bits(&want));
                }
            }
        }
    }

    #[test]
    fn identity_density_model_renders_as_no_model() {
        let img = textured_negative(1100, 700, [0.25, 0.55, 0.8], [1.6, 1.8, 2.0]);
        let plain = render_frame(&img, &Settings::default(), None).unwrap();
        let ctx = RenderContext {
            density: Some(&DensityModel::IDENTITY),
            ..RenderContext::default()
        };
        let model = render_frame_with(&img, &Settings::default(), &ctx, None).unwrap();
        let (a, b) = (plain.estimates, model.estimates);
        assert_eq!(a.dmin_source, b.dmin_source);
        assert_eq!(a.cast_source, b.cast_source);
        for c in 0..3 {
            assert!((a.dmin[c] - b.dmin[c]).abs() < 1e-5, "{a:?} {b:?}");
            assert!((a.dmax[c] - b.dmax[c]).abs() < 1e-5, "{a:?} {b:?}");
        }
        assert!((a.anchor - b.anchor).abs() < 1e-4);
        for (x, y) in plain.image.data().iter().zip(model.image.data()) {
            assert!((x - y).abs() < 1e-4, "{x} {y}");
        }
    }

    #[test]
    fn density_model_drives_the_grid_estimates() {
        // A model with a 0.4 offset in every channel and doubled slope in
        // green: Dmin and Dmax follow the model, not the plain formula.
        let img = synthetic_negative([0.2, 0.5, 0.7], [1.7, 1.7, 1.6]);
        let model = DensityModel {
            a: [0.4; 3],
            g: [[-1.0, 0.0, 0.0], [0.0, -2.0, 0.0], [0.0, 0.0, -1.0]],
        };
        let ctx = RenderContext {
            density: Some(&model),
            ..RenderContext::default()
        };
        let plain = render_frame(&img, &Settings::default(), None).unwrap();
        let r = render_frame_with(&img, &Settings::default(), &ctx, None).unwrap();
        let (p, m) = (plain.estimates, r.estimates);
        assert!((m.dmin[0] - (p.dmin[0] + 0.4)).abs() < 1e-4, "{p:?} {m:?}");
        assert!(
            (m.dmin[1] - (2.0 * p.dmin[1] + 0.4)).abs() < 1e-4,
            "{p:?} {m:?}"
        );
        assert!(
            (m.dmax[1] - (2.0 * p.dmax[1] + 0.4)).abs() < 1e-3,
            "{p:?} {m:?}"
        );
        // The offset cancels in d = D - Dmin and the doubled slope cancels
        // in d / gamma, so the render is the same within rounding.
        for (x, y) in plain.image.data().iter().zip(r.image.data()) {
            assert!((x - y).abs() < 1e-4, "{x} {y}");
        }
    }

    /// The grid, its plain density and the estimates under `settings` and
    /// `ctx`.
    fn estimate(img: &Image, settings: &Settings, ctx: &RenderContext) -> Result<Estimates> {
        let grid = analysis_grid(img, ANALYSIS_GRID_LONG_SIDE);
        let d = density_grid(&grid.median, ctx.density);
        estimate_frame(&grid, &d, settings, ctx)
    }

    fn film(ratio: [f32; 3], offset: [f32; 3]) -> FilmCorrection {
        FilmCorrection {
            name: "test film".to_string(),
            rig: None,
            gamma_ratio: ratio,
            offset_log_d: offset,
        }
    }

    #[test]
    fn dmin_and_dmax_precedence() {
        let img = synthetic_negative([0.2, 0.5, 0.7], [1.7, 1.7, 1.6]);
        let own = estimate(&img, &Settings::default(), &RenderContext::default()).unwrap();
        assert!(matches!(
            own.dmin_source,
            DminSource::Border | DminSource::Percentile
        ));
        let pooled = PooledEstimates {
            dmin: Some([0.3, 0.6, 0.8]),
            ..PooledEstimates::default()
        };
        let ctx = RenderContext {
            pooled: Some(&pooled),
            ..RenderContext::default()
        };
        // Pooled beats the estimate.
        let e = estimate(&img, &Settings::default(), &ctx).unwrap();
        assert_eq!(e.dmin, [0.3, 0.6, 0.8]);
        assert_eq!(e.dmin_source, DminSource::Roll);
        assert_eq!(e.dmax, own.dmax);
        assert_eq!(e.gamma, no_profile_gamma([0.3, 0.6, 0.8], own.dmax));
        // Fixed beats the pool.
        let mut s = Settings::default();
        s.film.film_base = ChannelEstimate::Fixed {
            value: [0.1, 0.2, 0.3],
        };
        s.film.dmax = ChannelEstimate::Fixed {
            value: [2.0, 2.1, 2.2],
        };
        let e = estimate(&img, &s, &ctx).unwrap();
        assert_eq!(e.dmin, [0.1, 0.2, 0.3]);
        assert_eq!(e.dmin_source, DminSource::Fixed);
        assert_eq!(e.dmax, [2.0, 2.1, 2.2]);
        assert_eq!(e.gamma, no_profile_gamma([0.1, 0.2, 0.3], [2.0, 2.1, 2.2]));
    }

    #[test]
    fn gamma_and_neutral_offset_precedence() {
        let img = synthetic_negative([0.2, 0.5, 0.7], [1.7, 1.7, 1.6]);
        let own = estimate(&img, &Settings::default(), &RenderContext::default()).unwrap();
        assert_eq!(own.gamma_source, GammaSource::Range);
        assert_eq!(own.neutral_offset, [0.0; 3]);
        let range = no_profile_gamma(own.dmin, own.dmax);
        assert_eq!(own.gamma, range);

        let pooled = PooledEstimates {
            gamma: Some([1.5, 1.6, 1.4]),
            ..PooledEstimates::default()
        };
        let f = film([0.9, 1.0, 1.2], [0.03, 0.0, -0.02]);

        // Pooled beats the range.
        let ctx = RenderContext {
            pooled: Some(&pooled),
            ..RenderContext::default()
        };
        let e = estimate(&img, &Settings::default(), &ctx).unwrap();
        assert_eq!(e.gamma, [1.5, 1.6, 1.4]);
        assert_eq!(e.gamma_source, GammaSource::Roll);
        assert_eq!(e.neutral_offset, [0.0; 3]);

        // Film beats the pool, with gamma_G from the pool.
        let ctx = RenderContext {
            film: Some(&f),
            pooled: Some(&pooled),
            ..RenderContext::default()
        };
        let e = estimate(&img, &Settings::default(), &ctx).unwrap();
        assert_eq!(e.gamma, [0.9 * 1.6, 1.6, 1.2 * 1.6]);
        assert_eq!(e.gamma_source, GammaSource::Profile);
        assert_eq!(e.neutral_offset, [0.03 / 1.6, 0.0, -0.02 / 1.6]);

        // Film without a pool takes gamma_G from the frame's green range.
        let ctx = RenderContext {
            film: Some(&f),
            ..RenderContext::default()
        };
        let e = estimate(&img, &Settings::default(), &ctx).unwrap();
        let g = range[1];
        assert_eq!(e.gamma, [0.9 * g, g, 1.2 * g]);
        assert_eq!(e.gamma_source, GammaSource::Profile);
        assert_eq!(e.neutral_offset, [0.03 / g, 0.0, -0.02 / g]);

        // Fixed beats the film; the offsets still apply over the fixed G.
        let mut s = Settings::default();
        s.film.gamma = ChannelEstimate::Fixed {
            value: [1.2, 1.25, 1.3],
        };
        let ctx = RenderContext {
            film: Some(&f),
            pooled: Some(&pooled),
            ..RenderContext::default()
        };
        let e = estimate(&img, &s, &ctx).unwrap();
        assert_eq!(e.gamma, [1.2, 1.25, 1.3]);
        assert_eq!(e.gamma_source, GammaSource::Fixed);
        assert_eq!(e.neutral_offset, [0.03 / 1.25, 0.0, -0.02 / 1.25]);

        // A bad film correction is an error.
        for bad in [
            film([0.0, 1.0, 1.0], [0.0; 3]),
            film([1.0, 1.0, f32::NAN], [0.0; 3]),
            film([1.0; 3], [f32::INFINITY, 0.0, 0.0]),
        ] {
            let ctx = RenderContext {
                film: Some(&bad),
                ..RenderContext::default()
            };
            assert!(matches!(
                estimate(&img, &Settings::default(), &ctx),
                Err(Error::Invalid(_))
            ));
        }
    }

    #[test]
    fn cast_precedence() {
        let img = synthetic_negative([0.2, 0.5, 0.7], [1.7, 1.7, 1.6]);
        let fixed = CastEstimate::Fixed {
            slope_r: 1.1,
            offset_r: 0.01,
            slope_b: 0.9,
            offset_b: -0.02,
        };
        let fixed_cast = CastCorrection {
            slope_r: 1.1,
            offset_r: 0.01,
            slope_b: 0.9,
            offset_b: -0.02,
            cells_used: 0,
        };
        let pooled_cast = CastCorrection {
            slope_r: 1.05,
            offset_r: 0.0,
            slope_b: 0.95,
            offset_b: 0.01,
            cells_used: 500,
        };
        let pooled = PooledEstimates {
            cast: Some(pooled_cast),
            ..PooledEstimates::default()
        };
        let f = film([1.0; 3], [0.0; 3]);
        let none = RenderContext::default();
        let with_pool = RenderContext {
            pooled: Some(&pooled),
            ..none
        };
        let with_film = RenderContext {
            film: Some(&f),
            pooled: Some(&pooled),
            ..none
        };

        // Fit by default.
        let e = estimate(&img, &Settings::default(), &none).unwrap();
        assert_eq!(e.cast_source, CastSource::Fit);
        assert!(e.cast.cells_used > 0);
        // Pool beats the fit.
        let e = estimate(&img, &Settings::default(), &with_pool).unwrap();
        assert_eq!((e.cast, e.cast_source), (pooled_cast, CastSource::Roll));
        // Auto colour off or a film correction beat the pool: identity.
        let mut off = Settings::default();
        off.colour.auto_colour = false;
        let e = estimate(&img, &off, &with_pool).unwrap();
        assert_eq!(
            (e.cast, e.cast_source),
            (CastCorrection::IDENTITY, CastSource::Identity)
        );
        let e = estimate(&img, &Settings::default(), &with_film).unwrap();
        assert_eq!(
            (e.cast, e.cast_source),
            (CastCorrection::IDENTITY, CastSource::Identity)
        );
        // Fixed beats everything, auto colour off and film included.
        for auto in [true, false] {
            let mut s = Settings::default();
            s.colour.cast = fixed;
            s.colour.auto_colour = auto;
            for ctx in [&none, &with_pool, &with_film] {
                let e = estimate(&img, &s, ctx).unwrap();
                assert_eq!((e.cast, e.cast_source), (fixed_cast, CastSource::Fixed));
            }
        }
        // A fit with fewer than 8 surviving cells reports Identity: a 2x2
        // frame has only 4 grid cells.
        let mut tiny = Image::new(2, 2);
        for (i, v) in tiny.data_mut().iter_mut().enumerate() {
            *v = 0.1 + 0.05 * i as f32;
        }
        let e = estimate(&tiny, &Settings::default(), &none).unwrap();
        assert_eq!(e.cast.cells_used, 0);
        assert_eq!(e.cast_source, CastSource::Identity);
    }

    #[test]
    fn film_correction_renders_a_synthetic_negative_neutral() {
        // A negative whose layers have gammas (1.6, 1.8, 2.1) and whose R
        // and B carry the log-D offsets the film correction measured: green
        // density above base is s, and d_c = ratio_c * (s - offset_c). The
        // grid equals the frame (256 x 192 < 1024), every cv is zero and
        // the border is the base, so Dmin is exact.
        let gammas = [1.6f32, 1.8, 2.1];
        let ratio = [gammas[0] / gammas[1], 1.0, gammas[2] / gammas[1]];
        let offset = [-0.04f32, 0.0, -0.02];
        let base = [0.2f32, 0.5, 0.7];
        let border = 24;
        let mut img = Image::new(W, H);
        for y in 0..H {
            for x in 0..W {
                let interior = x >= border && x < W - border && y >= border && y < H - border;
                let mut px = base.map(|d| 10f32.powf(-d));
                if interior {
                    let s = gammas[1] * (x - border) as f32 / (W - 2 * border) as f32 * 0.9;
                    for c in 0..3 {
                        let d = ratio[c] * (s - offset[c]);
                        px[c] = 10f32.powf(-(base[c] + d));
                    }
                }
                img.set_pixel(x, y, px);
            }
        }
        let f = film(ratio, offset);
        let ctx = RenderContext {
            film: Some(&f),
            ..RenderContext::default()
        };
        let r = render_frame_with(&img, &Settings::default(), &ctx, None).unwrap();
        let est = r.estimates;
        assert_eq!(est.gamma_source, GammaSource::Profile);
        assert_eq!(est.cast_source, CastSource::Identity);
        for (got, want) in est.dmin.iter().zip(base) {
            assert!((got - want).abs() < 1e-6, "{est:?}");
        }
        // e per interior pixel as render_full computes it.
        for y in border..H - border {
            for x in border..W - border {
                let d = img.pixel(x, y).map(negg_film::density_value);
                let e = crate::invert_pixel(d, est.dmin, est.gamma);
                let e = [
                    e[0] - est.neutral_offset[0],
                    e[1] - est.neutral_offset[1],
                    e[2] - est.neutral_offset[2],
                ];
                assert!((e[0] - e[1]).abs() < 1e-4, "{x},{y}: {e:?}");
                assert!((e[2] - e[1]).abs() < 1e-4, "{x},{y}: {e:?}");
                let out = r.image.pixel(x, y);
                assert!(
                    (out[0] - out[1]).abs() < 1e-3 && (out[2] - out[1]).abs() < 1e-3,
                    "{x},{y}: {out:?}"
                );
            }
        }
        // Without the correction the same negative is not neutral before
        // the cast fit: the range gamma differs per channel.
        let plain = estimate(&img, &Settings::default(), &RenderContext::default()).unwrap();
        assert_eq!(plain.gamma_source, GammaSource::Range);
    }

    #[test]
    fn estimate_frame_rejects_bad_inputs() {
        let img = synthetic_negative([0.3; 3], [1.6; 3]);
        let grid = analysis_grid(&img, ANALYSIS_GRID_LONG_SIDE);
        let ctx = RenderContext::default();
        let wrong = Image::new(grid.width() - 1, grid.height());
        assert!(matches!(
            estimate_frame(&grid, &wrong, &Settings::default(), &ctx),
            Err(Error::Invalid(_))
        ));
        let empty = analysis_grid(&Image::new(0, 0), ANALYSIS_GRID_LONG_SIDE);
        assert!(matches!(
            estimate_frame(&empty, &Image::new(0, 0), &Settings::default(), &ctx),
            Err(Error::Invalid(_))
        ));
        let mut bad = Settings::default();
        bad.print.grade = 500.0;
        let d = density_grid(&grid.median, None);
        assert!(matches!(
            estimate_frame(&grid, &d, &bad, &ctx),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn invalid_settings_empty_image_and_unreachable_anchor_are_errors() {
        let img = synthetic_negative([0.3; 3], [1.6; 3]);
        let mut settings = Settings::default();
        settings.print.grade = 500.0;
        assert!(matches!(
            render_frame(&img, &settings, None),
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            render_frame(&Image::new(0, 10), &Settings::default(), None),
            Err(Error::Invalid(_))
        ));
        // Paper white denser than the 0.75 anchor density: P = 0.75 is
        // unreachable, x0 would be NaN, and it must not be stored.
        let mut settings = Settings::default();
        settings.print.paper_dmin = 0.8;
        match render_frame(&img, &settings, None) {
            Err(Error::Invalid(reason)) => assert!(reason.contains("x0"), "{reason}"),
            other => panic!("expected Error::Invalid, got {other:?}"),
        }
    }
}
