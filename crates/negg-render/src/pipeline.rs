// SPDX-License-Identifier: MPL-2.0

//! Steps 3 to 7 of `docs/MATHS.md` orchestrated for one frame.
//!
//! [`render_frame_with`] is the one pipeline of ground rule 3: it builds
//! the analysis grid and its density (step 3, with the rig density model
//! when the [`RenderContext`] carries one), runs [`estimate_frame`] (Dmin,
//! Dmax and gamma by the lock precedence of ADR 0017, the film correction's
//! neutral offset, the cast by its source (ADR 0018), the exposure anchor and `x_0`), then
//! renders the full-resolution frame in one fused pass with
//! [`render_full`] (steps 3 to 7: the density is taken per pixel inside the
//! kernel). [`render_frame`] is the same with the default context, which
//! renders exactly as phase 1. The estimators themselves live in
//! `negg-film`; this module only calls them in order. It does no I/O and
//! no colour management: the matrix of step 7 is chosen by the
//! caller with `negg-cms` and passed in. The linear part of step 2 (flat
//! field, unmix) is applied by the caller beforehand with
//! [`crate::apply_rig_linear`].
//!
//! Memory: besides the input, one full-size `f32` image is allocated, the
//! output of [`render_full`]; no full-size density image is made. The
//! grid-sized intermediates are about a thousand
//! pixels on the long side and are not counted.

use std::borrow::Cow;

use negg_film::{
    AnalysisGrid, DminEstimate, analysis_grid, estimate_dmax, estimate_dmin, exposure_anchor,
    grid_cells_in, no_profile_gamma, sub_grid,
};
use negg_types::settings::ChannelEstimate;
use negg_types::{
    CastCorrection, CastEstimate, CastSource, DensityModel, DminSource, Error, Estimates,
    FilmCorrection, GammaSource, Geometry, Image, PooledEstimates, Result, Settings,
};

use crate::cast::{apply_cast, fit_cast};
use crate::full::render_full;
use crate::invert::invert;
use crate::print::{PrintCoefficients, solve_exposure};
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
    /// when `camera_to_working` was `Some`, otherwise the input's own linear
    /// RGB.
    pub image: Image,
    /// Steps 3 to 6: what was estimated or fixed for this frame, as written
    /// to the sidecar (step 8).
    pub estimates: Estimates,
    /// Number of cells in the analysis grid of step 3: the whole grid
    /// without an analysis area or a crop, else the buffered analysis grid.
    pub grid_cells: u32,
    /// Step 5 (ADR 0030): true when the black-point floor raised `x_0`
    /// above the anchor's, so the frame prints darker than the anchor alone
    /// would make it. Derived from `estimates.anchor` and the print
    /// settings; not stored in the sidecar.
    pub black_point_floor: bool,
}

/// What a frame is rendered with besides its own pixels and settings
/// (steps 2 to 6 of `docs/MATHS.md`, phase 2): the rig density model, the
/// film correction and the roll's pooled estimates. Every part is
/// optional; the default context renders exactly as phase 1.
#[derive(Clone, Copy, Debug, Default)]
pub struct RenderContext<'a> {
    /// The rig density model replacing the step 3 density.
    pub density: Option<&'a DensityModel>,
    /// The film correction: gamma ratios and neutral offsets.
    pub film: Option<&'a FilmCorrection>,
    /// The roll's pooled Dmin, gamma and cast, already filtered by the
    /// roll's `pool_*` flags.
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
    let g = model_grid(grid, grid_density, model);
    (estimate_dmin(&g), estimate_dmax(&g))
}

/// The grid the step 3 estimators run on: `grid` itself without a density
/// model, else a grid whose block medians are `10^-D` of `grid_density`
/// and whose `cv`, block and source size are `grid`'s (see
/// [`estimate_dmin_dmax`]).
fn model_grid<'a>(
    grid: &'a AnalysisGrid,
    grid_density: &Image,
    model: Option<&DensityModel>,
) -> Cow<'a, AnalysisGrid> {
    match model {
        None => Cow::Borrowed(grid),
        Some(_) => {
            let data = grid_density.data().iter().map(|d| 10f32.powf(-d)).collect();
            let median = Image::from_vec(grid_density.width(), grid_density.height(), data)
                .expect("same shape as the density grid");
            Cow::Owned(AnalysisGrid {
                median,
                cv: grid.cv.clone(),
                block: grid.block,
                source_width: grid.source_width,
                source_height: grid.source_height,
            })
        }
    }
}

/// Fewest cells the analysis grid of a cropped frame may have.
/// Provisional choice.
pub const MIN_ANALYSIS_CELLS: usize = 64;

/// The analysis area of step 3 of `docs/MATHS.md` for a frame with an
/// analysis rectangle, its analysis area or else its crop (91
/// and 134): the crop grid (cells whose centres lie in the unbuffered
/// rectangle) and the analysis grid (cells whose centres lie in the
/// rectangle shrunk by the analysis buffer), each with its part of the grid
/// density.
pub(crate) struct AnalysisArea {
    /// The crop grid (the unbuffered analysis rectangle): the step 3 Dmin
    /// border search and percentile.
    pub crop: AnalysisGrid,
    /// The grid density of the crop grid.
    pub crop_density: Image,
    /// The buffered analysis grid: Dmax, the anchor and the cast fit.
    pub analysis: AnalysisGrid,
    /// The grid density of the analysis grid.
    pub analysis_density: Image,
}

/// Cuts the cells `cells` out of `grid` and its density; `too_small` is
/// the error message for an empty cut.
fn cut(
    grid: &AnalysisGrid,
    grid_density: &Image,
    cells: (usize, usize, usize, usize),
    too_small: &str,
) -> Result<(AnalysisGrid, Image)> {
    let sub = sub_grid(grid, cells);
    if sub.cells() == 0 {
        return Err(Error::Invalid(too_small.to_string()));
    }
    let density = grid_density.crop(cells.0, cells.1, sub.width(), sub.height())?;
    Ok((sub, density))
}

/// The analysis area of `grid` under `geometry`,
/// on [`Geometry::analysis_rect`] (the analysis area when set, else the
/// crop), or `None` when there is neither: the whole grid is then used
/// with no copy and the analysis buffer is ignored.
///
/// # Errors
///
/// [`Error::Invalid`] when the geometry fails validation, `grid_density`
/// does not match the grid, or the analysis grid has fewer than
/// [`MIN_ANALYSIS_CELLS`] cells ("crop too small to analyse", which
/// suggests an analysis area, or "analysis area too small to analyse" when
/// an analysis area is set).
pub(crate) fn analysis_area(
    grid: &AnalysisGrid,
    grid_density: &Image,
    geometry: &Geometry,
) -> Result<Option<AnalysisArea>> {
    if geometry.analysis_rect().is_none() {
        return Ok(None);
    }
    let too_small = if geometry.analysis_area.is_some() {
        "analysis area too small to analyse"
    } else {
        "crop too small to analyse; set an analysis area (--analysis-area, or the app's \
         analysis area tool) larger than the crop to analyse more of the film"
    };
    geometry.validate()?;
    if grid_density.width() != grid.width() || grid_density.height() != grid.height() {
        return Err(Error::Invalid(format!(
            "grid density is {}x{} but the analysis grid is {}x{}",
            grid_density.width(),
            grid_density.height(),
            grid.width(),
            grid.height()
        )));
    }
    let (w, h) = (grid.source_width, grid.source_height);
    let analysis_cells = grid_cells_in(grid, geometry.analysis_pixels(w, h));
    let count = (analysis_cells.2 - analysis_cells.0) * (analysis_cells.3 - analysis_cells.1);
    if count < MIN_ANALYSIS_CELLS {
        return Err(Error::Invalid(too_small.to_string()));
    }
    let (analysis, analysis_density) = cut(grid, grid_density, analysis_cells, too_small)?;
    let crop_cells = grid_cells_in(grid, geometry.analysis_rect_pixels(w, h));
    let (crop, crop_density) = cut(grid, grid_density, crop_cells, too_small)?;
    Ok(Some(AnalysisArea {
        crop,
        crop_density,
        analysis,
        analysis_density,
    }))
}

/// Step 3 Dmin and Dmax of `grid` under `geometry` (91 and
/// 134): with no analysis rectangle, [`estimate_dmin_dmax`] on the whole
/// grid exactly as before; with one, Dmin on the unbuffered rectangle's
/// grid and Dmax on the buffered analysis grid.
pub(crate) fn area_dmin_dmax(
    grid: &AnalysisGrid,
    grid_density: &Image,
    area: Option<&AnalysisArea>,
    model: Option<&DensityModel>,
) -> (DminEstimate, [f32; 3]) {
    match area {
        None => estimate_dmin_dmax(grid, grid_density, model),
        Some(a) => {
            let crop = model_grid(&a.crop, &a.crop_density, model);
            let analysis = model_grid(&a.analysis, &a.analysis_density, model);
            (estimate_dmin(&crop), estimate_dmax(&analysis))
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

/// Step 3 of `docs/MATHS.md`, roll mode: the Dmin a frame whose film base
/// is not fixed renders with, given its own estimate `own` and the roll's
/// pooled Dmin `pooled` (ADR 0029).
///
/// - No pooled Dmin: the frame's own estimate and source.
/// - A frame whose own source is not `Percentile` (a Border frame, Rejected
///   or not): the pooled value, source `Roll`.
/// - A `Percentile` frame: per channel `min(own_c, pooled_c)`, so the pool
///   never lifts its base above its own estimate. The source is `Roll`
///   when every channel took the pooled value (ties count as pooled),
///   `Percentile` when every channel took its own, else `RollMin`.
///
/// [`crate::pool_roll`] uses this for the Dmin in effect of each frame, so
/// the gamma it pools and the cast it fits are taken from the same Dmin the
/// frame is rendered with by [`estimate_frame`].
pub(crate) fn dmin_with_pool(
    own: &DminEstimate,
    pooled: Option<[f32; 3]>,
) -> ([f32; 3], DminSource) {
    let Some(pooled) = pooled else {
        return (own.value, own.source);
    };
    if own.source != DminSource::Percentile {
        return (pooled, DminSource::Roll);
    }
    let took_own = (0..3).any(|c| own.value[c] < pooled[c]);
    let took_pool = (0..3).any(|c| pooled[c] < own.value[c]);
    match (took_own, took_pool) {
        (false, _) => (pooled, DminSource::Roll),
        (true, false) => (own.value, DminSource::Percentile),
        (true, true) => (
            [
                own.value[0].min(pooled[0]),
                own.value[1].min(pooled[1]),
                own.value[2].min(pooled[2]),
            ],
            DminSource::RollMin,
        ),
    }
}

/// Steps 3 to 6 of `docs/MATHS.md` for one frame on its analysis grid:
/// every estimate the render needs, by the precedence rules of ADR 0017
/// (locks and overrides), ADR 0016 (film contrast) and ADR 0018 (cast
/// source).
///
/// `grid_density` must be the density of `grid.median` as step 3 defines
/// it for this context, that is `density_grid(&grid.median, ctx.density)`.
///
/// - Dmin: settings Fixed (`Fixed`) > `ctx.pooled` Dmin, capped for a
///   percentile frame by `dmin_with_pool` (`Roll`, `RollMin` or
///   `Percentile`) > the frame's estimate (`Border` or
///   `Percentile`).
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
/// - The anchor and `x_0`: [`exposure_anchor`] on the inverted, offset and
///   cast-corrected grid, then [`solve_exposure`]: the anchor's
///   [`crate::solve_x0`] floored by the black point of ADR 0030 so that the film
///   base prints at least at the knee of the toe, `P = Dmax_p - ln(2) / a_sh`
///   (ADR 0031).
///
/// # Errors
///
/// [`Error::Invalid`] when the settings fail validation, the grid has no
/// cells, `grid_density` does not match the grid, the film correction has
/// a non-finite or non-positive ratio or a non-finite offset, the gamma in
/// effect is not finite and positive, the exposure anchor cannot be
/// printed at `P = 0.75` with the paper densities in `settings.print`, or
/// a crop leaves fewer than [`MIN_ANALYSIS_CELLS`] analysis cells ("crop
/// too small to analyse").
pub fn estimate_frame(
    grid: &AnalysisGrid,
    grid_density: &Image,
    settings: &Settings,
    ctx: &RenderContext,
) -> Result<Estimates> {
    estimate_frame_and_floor(grid, grid_density, settings, ctx).map(|(estimates, _)| estimates)
}

/// [`estimate_frame`] plus whether the black-point floor of step 5
/// (ADR 0030) raised `x_0` above the anchor's: the `floor_active` of the
/// one [`solve_exposure`] call that produced `x_0`, so [`plan_frame`] and
/// [`render_frame_with`] never solve the exposure a second time.
fn estimate_frame_and_floor(
    grid: &AnalysisGrid,
    grid_density: &Image,
    settings: &Settings,
    ctx: &RenderContext,
) -> Result<(Estimates, bool)> {
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

    // Step 3: the analysis area. Without an
    // analysis area or a crop the whole grid is used, with no copy, exactly
    // as before.
    let area = analysis_area(grid, grid_density, &settings.geometry)?;
    let (a_grid, a_density) = match &area {
        Some(a) => (&a.analysis, &a.analysis_density),
        None => (grid, grid_density),
    };

    // Step 3: Dmin and Dmax. The estimators run only when a value is needed.
    let needs_estimate = matches!(settings.film.film_base, ChannelEstimate::Auto)
        || matches!(settings.film.dmax, ChannelEstimate::Auto);
    let estimated =
        needs_estimate.then(|| area_dmin_dmax(grid, grid_density, area.as_ref(), ctx.density));
    let (dmin, dmin_source) = match (settings.film.film_base, estimated) {
        (ChannelEstimate::Fixed { value }, _) => (value, DminSource::Fixed),
        (ChannelEstimate::Auto, Some((est, _))) => dmin_with_pool(&est, pooled.dmin),
        (ChannelEstimate::Auto, None) => unreachable!("estimated when Auto"),
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
    let mut e_grid = invert(a_density, dmin, gamma)?;
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

    // Step 5: the exposure anchor and x_0, floored by the black point
    // (ADR 0030).
    let anchor = exposure_anchor(a_grid, &e_grid);
    let solved = solve_exposure(anchor, &settings.print);
    let x0 = solved.x0;
    if !x0.is_finite() {
        return Err(Error::Invalid(format!(
            "exposure anchor {anchor} cannot be printed at P = 0.75 with paper_dmin {} \
             and paper_dmax {}: x0 is not finite",
            settings.print.paper_dmin, settings.print.paper_dmax
        )));
    }
    Ok((
        Estimates {
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
        },
        solved.floor_active,
    ))
}

/// Number of cells of the analysis grid of `grid` under `geometry`: the
/// whole grid without an analysis area or a crop, else the buffered
/// analysis grid.
fn analysis_cell_count(grid: &AnalysisGrid, geometry: &Geometry) -> usize {
    if geometry.analysis_rect().is_none() {
        return grid.cells();
    }
    let (cx0, cy0, cx1, cy1) = grid_cells_in(
        grid,
        geometry.analysis_pixels(grid.source_width, grid.source_height),
    );
    (cx1 - cx0) * (cy1 - cy0)
}

/// Everything steps 3 to 6 of `docs/MATHS.md` decide for one frame before
/// any full-resolution pixel is touched: the estimates, the step 5 print
/// coefficients built from them, the analysis cell count and the
/// black-point floor flag. Made by [`plan_frame`] from the frame's analysis
/// grid; [`render_full`] and [`crate::render_region`] render with it, so
/// the export and every preview tile of the same frame and settings share
/// one plan (Step 8 of docs/MATHS.md (preview)).
#[derive(Clone, Debug, PartialEq)]
pub struct FramePlan {
    /// Steps 3 to 6: what was estimated or fixed for this frame, as
    /// [`estimate_frame`] returns it.
    pub estimates: Estimates,
    /// Step 5: [`PrintCoefficients::new`] of the print settings and
    /// `estimates.x0`.
    pub coefficients: PrintCoefficients,
    /// Number of cells in the analysis grid of step 3: the whole grid
    /// without an analysis area or a crop, else the buffered analysis grid.
    pub grid_cells: u32,
    /// Step 5 (ADR 0030): true when the black-point floor raised `x_0`
    /// above the anchor's. Taken from the same exposure solve that set
    /// `estimates.x0`; not stored in the sidecar.
    pub black_point_floor: bool,
}

/// Steps 3 to 6 of `docs/MATHS.md` for one frame on its analysis grid,
/// plus the step 5 print coefficients: [`estimate_frame`] (with the same
/// precedence, errors and analysis area) followed by
/// [`PrintCoefficients::new`] of `settings.print` and the `x_0` it found.
///
/// `grid` must be [`analysis_grid`] of the frame's linear image at
/// [`ANALYSIS_GRID_LONG_SIDE`] and `grid_density` its density,
/// `density_grid(&grid.median, ctx.density)`. Rendering the frame with the
/// returned plan through [`render_full`] is exactly [`render_frame_with`].
///
/// # Errors
///
/// Every error of [`estimate_frame`] and of [`PrintCoefficients::new`].
pub fn plan_frame(
    grid: &AnalysisGrid,
    grid_density: &Image,
    settings: &Settings,
    ctx: &RenderContext,
) -> Result<FramePlan> {
    let (estimates, black_point_floor) =
        estimate_frame_and_floor(grid, grid_density, settings, ctx)?;
    let grid_cells =
        u32::try_from(analysis_cell_count(grid, &settings.geometry)).unwrap_or(u32::MAX);
    let coefficients = PrintCoefficients::new(&settings.print, estimates.x0)?;
    Ok(FramePlan {
        estimates,
        coefficients,
        grid_cells,
        black_point_floor,
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
///    (the rig model `ctx.density` when given).
/// 3. Steps 3 to 6 and the step 5 coefficients: [`plan_frame`]
///    ([`estimate_frame`]: Dmin, Dmax, gamma, neutral offset, cast, anchor
///    and `x_0` by the lock precedence of ADR 0017; then [`PrintCoefficients::new`]).
/// 4. Steps 3 to 7 fused on the full frame: [`render_full`] on `linear`
///    with `ctx.density` (the density of step 3 taken per pixel) and
///    `camera_to_working`, the step 7 matrix into the working space chosen by
///    the caller with `negg-cms`. Despite the name it is not a camera
///    matrix for RAW input: it holds the BT.2087 Rec.709-to-Rec.2020 matrix
///    there (ADR 0026) and the embedded profile's colorant matrix for a
///    tagged TIFF. `None` skips step 7, leaving the output in the input's
///    own RGB; a rig does not change it.
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
    let grid_density = density_grid(&grid.median, ctx.density);

    // Steps 3 to 6 on the grid, and the step 5 coefficients.
    let plan = plan_frame(&grid, &grid_density, settings, ctx)?;
    drop(grid_density);
    drop(grid);

    // Steps 3 to 7 fused on the full frame: the density is taken per pixel
    // inside the kernel, so the output is the only full-size allocation.
    let image = render_full(
        linear,
        &plan.estimates,
        &plan.coefficients,
        ctx.density,
        camera_to_working,
    )?;

    Ok(FrameRender {
        image,
        estimates: plan.estimates,
        grid_cells: plan.grid_cells,
        black_point_floor: plan.black_point_floor,
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
    use crate::print::{ANCHOR_PRINT_DENSITY, black_print_density, solve_x0};
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
        // With the anchor's own x0 (ADR 0008), a pixel whose e equals the
        // anchor prints at P = 0.75; the x0 in use is that x0 floored by the
        // black point (ADR 0030), so it prints there or darker.
        let est = r.estimates;
        let e = est.anchor;
        let x0_anchor = solve_x0(e, &PrintSettings::default());
        assert_eq!(
            est.x0,
            x0_anchor.max(crate::print::black_point_x0(&PrintSettings::default()))
        );
        assert_eq!(r.black_point_floor, est.x0 > x0_anchor);
        let d = est.dmin[1] - e * est.gamma[1];
        let mut one = Image::new(1, 1);
        one.set_pixel(0, 0, [10f32.powf(-d); 3]);
        let c = PrintCoefficients::new(&PrintSettings::default(), x0_anchor).unwrap();
        let out = render_full(&one, &est, &c, None, None).unwrap();
        let want = 10f32.powf(-ANCHOR_PRINT_DENSITY);
        assert!(
            ((out.pixel(0, 0)[1] - want) / want).abs() < 1e-3,
            "{:?} want {want}",
            out.pixel(0, 0)
        );
        let c = PrintCoefficients::new(&PrintSettings::default(), est.x0).unwrap();
        let used = render_full(&one, &est, &c, None, None).unwrap();
        assert!(used.pixel(0, 0)[1] <= want * (1.0 + 1e-3));
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
    /// `render_full` call gaining its `None` density argument and `x0`
    /// gaining the black-point floor of ADR 0030 (`max(x0, x_black)`): the
    /// default context must reproduce it bit for bit.
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
        let x0_anchor = solve_x0(anchor, &settings.print);
        let x_black = crate::print::black_point_x0(&settings.print);
        let black_point_floor = x_black > x0_anchor;
        let x0 = x0_anchor.max(x_black);
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
            black_point_floor,
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
    fn a_pooled_dmin_never_lifts_a_percentile_base() {
        let pooled = Some([0.3, 0.6, 0.8]);
        let est = |value, source| DminEstimate {
            value,
            source,
            flat_border_cells: 0,
        };
        // No pool: the frame's own estimate and source.
        let own = est([0.2, 0.5, 0.9], DminSource::Percentile);
        assert_eq!(
            dmin_with_pool(&own, None),
            ([0.2, 0.5, 0.9], DminSource::Percentile)
        );
        // A Border frame takes the pool even when it is higher.
        let border = est([0.2, 0.5, 0.7], DminSource::Border);
        assert_eq!(
            dmin_with_pool(&border, pooled),
            ([0.3, 0.6, 0.8], DminSource::Roll)
        );
        // A percentile frame takes the lower value per channel.
        let below = est([0.2, 0.5, 0.7], DminSource::Percentile);
        assert_eq!(
            dmin_with_pool(&below, pooled),
            ([0.2, 0.5, 0.7], DminSource::Percentile)
        );
        let above = est([0.4, 0.6, 0.9], DminSource::Percentile);
        assert_eq!(
            dmin_with_pool(&above, pooled),
            ([0.3, 0.6, 0.8], DminSource::Roll)
        );
        assert_eq!(
            dmin_with_pool(&own, pooled),
            ([0.2, 0.5, 0.8], DminSource::RollMin)
        );
    }

    #[test]
    fn a_percentile_frame_renders_below_a_higher_pool_with_its_own_base() {
        let mut img = Image::new(W, H);
        for y in 0..H {
            for x in 0..W {
                let t = x as f32 / (W - 1) as f32 * 0.9 + 0.08 * ((x / 3 + y / 3) % 2) as f32;
                let d = [0.2 + 1.6 * t, 0.5 + 1.7 * t, 0.7 + 1.8 * t];
                img.set_pixel(x, y, d.map(|d| 10f32.powf(-d)));
            }
        }
        let own = estimate(&img, &Settings::default(), &RenderContext::default()).unwrap();
        assert_eq!(own.dmin_source, DminSource::Percentile);
        let lifted = own.dmin.map(|d| d + 0.3);
        let pooled = PooledEstimates {
            dmin: Some(lifted),
            ..PooledEstimates::default()
        };
        let ctx = RenderContext {
            pooled: Some(&pooled),
            ..RenderContext::default()
        };
        let e = estimate(&img, &Settings::default(), &ctx).unwrap();
        assert_eq!((e.dmin, e.dmin_source), (own.dmin, DminSource::Percentile));
        assert_eq!(e.gamma, no_profile_gamma(own.dmin, own.dmax));
        // Fixed still wins over everything.
        let mut s = Settings::default();
        s.film.film_base = ChannelEstimate::Fixed { value: lifted };
        let e = estimate(&img, &s, &ctx).unwrap();
        assert_eq!((e.dmin, e.dmin_source), (lifted, DminSource::Fixed));
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

    /// `inner` placed at `(ox, oy)` on a `w` by `h` canvas whose surround
    /// is linear 1e-6 (density at the 6.0 cap), like a film holder in shot.
    fn in_surround(inner: &Image, w: usize, h: usize, ox: usize, oy: usize) -> Image {
        let mut img = Image::from_vec(w, h, vec![1e-6; w * h * 3]).unwrap();
        for y in 0..inner.height() {
            for x in 0..inner.width() {
                img.set_pixel(ox + x, oy + y, inner.pixel(x, y));
            }
        }
        img
    }

    fn cropped(x: f32, y: f32, width: f32, height: f32, buffer: f32) -> Settings {
        let mut s = Settings::default();
        s.geometry.crop = Some(negg_types::CropRect {
            x,
            y,
            width,
            height,
        });
        s.geometry.analysis_buffer = buffer;
        s
    }

    #[test]
    fn a_crop_to_the_film_removes_the_surround_from_the_analysis() {
        let a = synthetic_negative([0.2, 0.5, 0.7], [1.7, 1.7, 1.6]);
        let b = in_surround(&a, 400, 300, 64, 48);
        let own = render_frame(&a, &Settings::default(), None)
            .unwrap()
            .estimates;
        // Without a crop the surround pins Dmax at the cap.
        let wide = estimate(&b, &Settings::default(), &RenderContext::default()).unwrap();
        assert!(wide.dmax.iter().all(|d| (d - 6.0).abs() < 1e-3), "{wide:?}");
        let (x, y, w, h) = (64.0 / 400.0, 48.0 / 300.0, 256.0 / 400.0, 192.0 / 300.0);
        for (buffer, tol) in [(0.0, 0.01), (ANALYSIS_BUFFER_DEFAULT_F, 0.02)] {
            let s = cropped(x, y, w, h, buffer);
            assert_eq!(s.geometry.crop_pixels(400, 300), (64, 48, 320, 240));
            let e = estimate(&b, &s, &RenderContext::default()).unwrap();
            for c in 0..3 {
                assert!((e.dmin[c] - own.dmin[c]).abs() < tol, "{e:?} {own:?}");
                assert!((e.dmax[c] - own.dmax[c]).abs() < tol, "{e:?} {own:?}");
                assert!((e.gamma[c] - own.gamma[c]).abs() < tol, "{e:?} {own:?}");
            }
            assert_eq!(e.dmin_source, own.dmin_source);
        }
        // With no buffer the crop sees exactly A's grid.
        let e = estimate(&b, &cropped(x, y, w, h, 0.0), &RenderContext::default()).unwrap();
        assert_eq!(e, own);
        let r = render_frame(&b, &cropped(x, y, w, h, 0.0), None).unwrap();
        assert_eq!(r.grid_cells, (256 * 192) as u32);
        assert_eq!(r.estimates, own);
    }

    const ANALYSIS_BUFFER_DEFAULT_F: f32 = negg_types::ANALYSIS_BUFFER_DEFAULT;

    #[test]
    fn the_border_search_runs_inside_the_crop_only() {
        let a = synthetic_negative([0.2, 0.5, 0.7], [1.7, 1.7, 1.6]);
        let b = in_surround(&a, 400, 300, 64, 48);
        let ctx = RenderContext::default();
        // A crop that keeps the rebate keeps the border source.
        let keep = cropped(
            64.0 / 400.0,
            48.0 / 300.0,
            256.0 / 400.0,
            192.0 / 300.0,
            0.05,
        );
        assert_eq!(
            estimate(&b, &keep, &ctx).unwrap().dmin_source,
            DminSource::Border
        );
        // A crop inside the image area loses it: the percentile path.
        let tight = cropped(
            (64.0 + 30.0) / 400.0,
            (48.0 + 30.0) / 300.0,
            196.0 / 400.0,
            132.0 / 300.0,
            0.05,
        );
        assert_eq!(
            estimate(&b, &tight, &ctx).unwrap().dmin_source,
            DminSource::Percentile
        );
    }

    #[test]
    fn a_crop_under_64_analysis_cells_is_an_error() {
        let a = synthetic_negative([0.2, 0.5, 0.7], [1.7, 1.7, 1.6]);
        // 0.03 of 256 x 192 is about 8 x 6 = 48 cells.
        let s = cropped(0.5, 0.5, 0.03, 0.03, 0.0);
        match estimate(&a, &s, &RenderContext::default()) {
            Err(Error::Invalid(reason)) => {
                assert!(reason.starts_with("crop too small to analyse; set an analysis area"))
            }
            other => panic!("expected crop too small, got {other:?}"),
        }
        assert!(render_frame(&a, &s, None).is_err());
        assert!(crate::analyse_frame(&a, None, &s.geometry).is_err());
        // A larger crop passes.
        let ok = cropped(0.25, 0.25, 0.5, 0.5, 0.05);
        estimate(&a, &ok, &RenderContext::default()).unwrap();
    }

    #[test]
    fn analyse_frame_with_a_crop_matches_the_pipeline() {
        let a = synthetic_negative([0.2, 0.5, 0.7], [1.7, 1.7, 1.6]);
        let b = in_surround(&a, 400, 300, 64, 48);
        let s = cropped(0.1, 0.1, 0.7, 0.75, 0.05);
        let analysis = crate::analyse_frame(&b, None, &s.geometry).unwrap();
        let e = estimate(&b, &s, &RenderContext::default()).unwrap();
        assert_eq!(analysis.dmin.value, e.dmin);
        assert_eq!(analysis.dmin.source, e.dmin_source);
        assert_eq!(analysis.dmax, e.dmax);
        let (x0, y0, x1, y1) = s.geometry.analysis_pixels(400, 300);
        assert_eq!(
            (
                analysis.grid_density.width(),
                analysis.grid_density.height()
            ),
            (x1 - x0, y1 - y0)
        );
        // No crop: the whole grid, as before.
        let whole = crate::analyse_frame(&b, None, &Geometry::default()).unwrap();
        assert_eq!(whole.grid_density.width(), 400);
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

    /// A scene for the black-point floor (ADR 0030): an unexposed border of
    /// density 0.3 (the film base, `e = 0`) around a flat, dense sky over
    /// the top 70% of the interior (density `0.3 + 1.5 * 0.9`, which also
    /// sets Dmax) and a textured foreground below it whose density ramps
    /// from `0.3 + 1.5 * lo` to `0.3 + 1.5 * hi`, with a one-pixel checker
    /// of 0.08 log-D so that every 2 by 2 analysis block is textured.
    /// 1200 by 800, so the grid has real block medians. Neutral, so step 6
    /// leaves `e = 0` where it is.
    fn sky_scene(lo: f32, hi: f32) -> Image {
        let (w, h) = (1200, 800);
        let border = w / 10;
        let mut img = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let interior = x >= border && x < w - border && y >= border && y < h - border;
                let t = if !interior {
                    0.0
                } else if y < border + (h - 2 * border) * 7 / 10 {
                    0.9
                } else {
                    let ramp = (x - border) as f32 / (w - 2 * border) as f32;
                    let texture = if (x + y) % 2 == 0 { 0.0 } else { 0.08 };
                    lo + (hi - lo) * ramp + texture
                };
                img.set_pixel(x, y, [10f32.powf(-(0.3 + 1.5 * t)); 3]);
            }
        }
        img
    }

    /// Print density of a rendered value: `P = -log10(out)`.
    fn print_density(out: f32) -> f32 {
        -out.log10()
    }

    #[test]
    fn a_high_anchor_is_floored_so_the_base_prints_black() {
        let img = sky_scene(0.05, 0.25);
        let r = render_frame(&img, &Settings::default(), None).unwrap();
        let est = r.estimates;
        assert_eq!(est.dmin_source, DminSource::Border, "{est:?}");
        // The textured foreground sits close to the base, so the anchor is
        // high (near e = 0).
        assert!(est.anchor > -0.4, "anchor {}", est.anchor);
        let print = PrintSettings::default();
        let p_black = black_print_density(&print);
        // Before (ADR 0008 alone): the base prints well short of black.
        let before = PrintCoefficients::new(&print, solve_x0(est.anchor, &print)).unwrap();
        let p_before = print_density(crate::print_pixel([0.0; 3], &before)[1]);
        assert!(p_before < p_black - 0.3, "before {p_before}");
        // After: the floor is on and the base prints at P_black.
        assert!(r.black_point_floor);
        assert_eq!(est.x0, crate::print::black_point_x0(&print));
        for (x, y) in [(2, 2), (600, 40), (1190, 790)] {
            for ch in r.image.pixel(x, y) {
                let p = print_density(ch);
                assert!(
                    (p - p_black).abs() < 2e-3,
                    "base at ({x}, {y}) prints at P = {p}"
                );
            }
        }
        // The user's exposure still moves the base (it is added after the
        // floor): a negative exposure lifts it below P_black on purpose.
        let mut lighter = Settings::default();
        lighter.print.exposure = -0.3;
        let l = render_frame(&img, &lighter, None).unwrap();
        assert_eq!(l.estimates.x0, est.x0);
        assert!(print_density(l.image.pixel(2, 2)[1]) < p_black - 0.2);
    }

    #[test]
    fn a_low_anchor_renders_byte_identical_to_the_anchor_alone() {
        let img = sky_scene(0.55, 0.8);
        let r = render_frame(&img, &Settings::default(), None).unwrap();
        let est = r.estimates;
        let print = PrintSettings::default();
        let p_black = black_print_density(&print);
        let x0_anchor = solve_x0(est.anchor, &print);
        // The anchor already prints the base beyond P_black.
        assert!(!r.black_point_floor, "{est:?}");
        assert!(x0_anchor > crate::print::black_point_x0(&print));
        assert_eq!(est.x0.to_bits(), x0_anchor.to_bits());
        let c = PrintCoefficients::new(&print, x0_anchor).unwrap();
        let before = render_full(&img, &est, &c, None, None).unwrap();
        let bits = |i: &Image| -> Vec<u32> { i.data().iter().map(|v| v.to_bits()).collect() };
        assert_eq!(bits(&r.image), bits(&before));
        assert!(print_density(r.image.pixel(2, 2)[1]) > p_black);
    }

    #[test]
    fn plan_frame_then_render_full_is_render_frame_with() {
        let m = [[0.9, 0.1, 0.0], [0.05, 0.9, 0.05], [0.0, 0.2, 0.8]];
        let model = DensityModel {
            a: [0.02, -0.01, 0.03],
            g: [[-1.02, 0.03, 0.0], [0.02, -0.98, 0.01], [0.0, 0.04, -1.05]],
        };
        let correction = film([1.05, 1.0, 0.93], [0.02, 0.0, -0.03]);
        let pooled = PooledEstimates {
            dmin: Some([0.22, 0.52, 0.71]),
            gamma: Some([1.6, 1.7, 1.75]),
            cast: None,
        };
        // Every context, crop and matrix on the small frame; the default
        // context on two larger scenes, one either side of the black-point
        // floor, to keep debug runs short.
        let small = synthetic_negative([0.2, 0.5, 0.7], [1.7, 1.7, 1.6]);
        let floored = sky_scene(0.05, 0.25);
        let plain = sky_scene(0.55, 0.8);
        let crop = cropped(0.1, 0.12, 0.8, 0.75, 0.05);
        let default = Settings::default();
        let contexts = [
            RenderContext::default(),
            RenderContext {
                density: Some(&model),
                ..RenderContext::default()
            },
            RenderContext {
                film: Some(&correction),
                pooled: Some(&pooled),
                ..RenderContext::default()
            },
        ];
        type Case<'a> = (
            &'a Image,
            &'a Settings,
            &'a RenderContext<'a>,
            Option<[[f32; 3]; 3]>,
        );
        let mut cases: Vec<Case> = Vec::new();
        for settings in [&default, &crop] {
            for ctx in &contexts {
                for matrix in [None, Some(m)] {
                    cases.push((&small, settings, ctx, matrix));
                }
            }
        }
        cases.push((&floored, &default, &contexts[0], Some(m)));
        cases.push((&plain, &crop, &contexts[0], None));
        let mut floors = Vec::new();
        for (img, settings, ctx, matrix) in cases {
            let want = render_frame_with(img, settings, ctx, matrix).unwrap();
            let grid = analysis_grid(img, ANALYSIS_GRID_LONG_SIDE);
            let d = density_grid(&grid.median, ctx.density);
            let plan = plan_frame(&grid, &d, settings, ctx).unwrap();
            assert_eq!(plan.estimates, want.estimates);
            assert_eq!(
                plan.estimates,
                estimate_frame(&grid, &d, settings, ctx).unwrap()
            );
            assert_eq!(plan.grid_cells, want.grid_cells);
            assert_eq!(plan.black_point_floor, want.black_point_floor);
            // The flag is the one a second exposure solve gives.
            assert_eq!(
                plan.black_point_floor,
                solve_exposure(plan.estimates.anchor, &settings.print).floor_active
            );
            assert_eq!(
                plan.coefficients,
                PrintCoefficients::new(&settings.print, plan.estimates.x0).unwrap()
            );
            let got = render_full(
                img,
                &plan.estimates,
                &plan.coefficients,
                ctx.density,
                matrix,
            )
            .unwrap();
            let bits = |i: &Image| -> Vec<u32> { i.data().iter().map(|v| v.to_bits()).collect() };
            assert_eq!(bits(&got), bits(&want.image));
            floors.push(plan.black_point_floor);
        }
        // Both sides of the black-point floor were exercised.
        assert!(floors.contains(&true) && floors.contains(&false));
    }

    #[test]
    fn level_0_tiles_assemble_to_render_frame_with() {
        // Tiles of level 0 rendered with the frame plan are the
        // export render, pixel for pixel, including the ragged last row and
        // column of tiles.
        let img = textured_negative(1100, 700, [0.25, 0.55, 0.8], [1.6, 1.8, 2.0]);
        let m = [[0.9, 0.1, 0.0], [0.05, 0.9, 0.05], [0.0, 0.2, 0.8]];
        let settings = cropped(0.05, 0.05, 0.9, 0.9, 0.05);
        let ctx = RenderContext::default();
        let want = render_frame_with(&img, &settings, &ctx, Some(m)).unwrap();
        let grid = analysis_grid(&img, ANALYSIS_GRID_LONG_SIDE);
        let d = density_grid(&grid.median, ctx.density);
        let plan = plan_frame(&grid, &d, &settings, &ctx).unwrap();
        let mut assembled = Image::new(img.width(), img.height());
        for t in img.tiles(crate::TILE_SIZE) {
            let region = crate::Region {
                x: t.x,
                y: t.y,
                width: t.width,
                height: t.height,
            };
            let tile = crate::render_region(
                &img,
                region,
                &plan.estimates,
                &plan.coefficients,
                ctx.density,
                Some(m),
            )
            .unwrap();
            for y in 0..t.height {
                for x in 0..t.width {
                    assembled.set_pixel(t.x + x, t.y + y, tile.pixel(x, y));
                }
            }
        }
        let bits = |i: &Image| -> Vec<u32> { i.data().iter().map(|v| v.to_bits()).collect() };
        assert_eq!(bits(&assembled), bits(&want.image));
    }
}
