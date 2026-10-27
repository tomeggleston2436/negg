// SPDX-License-Identifier: Apache-2.0

//! Roll mode, profile loading and lock (ADR 0017;
//! ADR 0018).
//!
//! A roll is the set of batch inputs of one directory. `negg batch` renders
//! it in two passes: pass 1 decodes one frame at a time, applies the rig's
//! step 2 corrections and keeps only its analysis
//! ([`negg_render::analyse_frame`]); the frames are then pooled
//! ([`negg_render::pool_roll`]); pass 2 decodes each frame again and renders
//! it with the pooled values ([`render_in_roll`]). The roll sidecar
//! `roll.negg.json` beside the frames records the roll settings, the pool
//! and the frame list. `negg convert` uses an existing roll sidecar that
//! lists the frame, and never rewrites it. The two flows themselves are in
//! [`crate::flow`].
//!
//! Profile paths in `Settings.profiles` resolve relative to the directory
//! of the sidecar holding them, which is the frames' directory; paths
//! given with `--rig` and `--film` are made absolute before they are stored.
//!
//! Owns orchestration only; the maths is in `negg-render`, `negg-film` and
//! `negg-calib`.

use std::path::{Path, PathBuf};

use negg_decode::Transfer;
use negg_io::RollSidecar;
use negg_render::{
    FrameAnalysis, FrameRender, RenderContext, RollFrame, analyse_frame, apply_rig_linear,
    pool_roll, render_frame_with,
};
use negg_types::settings::ChannelEstimate;
use negg_types::{
    CastEstimate, Estimates, FilmCorrection, FrameOverrides, Image, RigCorrection, RollPool,
    RollSettings, Settings,
};

use crate::convert::{PreparedInput, prepare_input};
use crate::error::ConvertError;

/// Everything a roll's frames are rendered with besides their own pixels
/// and settings: the roll settings (shared settings and `pool_*` flags),
/// the rig correction and the film correction.
#[derive(Clone, Debug, PartialEq)]
pub struct RollContext {
    /// The roll settings; `shared.profiles` names the profiles loaded here.
    pub roll: RollSettings,
    /// The rig correction (step 2), when a rig profile is in use.
    pub rig: Option<RigCorrection>,
    /// The film correction (step 4), when a film profile with a `[gamma]`
    /// table is in use.
    pub film: Option<FilmCorrection>,
}

/// The directory a file lives in: its parent, or `.` for a bare name.
pub(crate) fn parent_dir(path: &Path) -> PathBuf {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// The file name of `path` as a string (empty when it has none).
pub(crate) fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A stored profile path resolved against `base_dir`, the directory of the
/// file that holds it. Absolute paths are kept as they are.
pub fn profile_path(base_dir: &Path, stored: &str) -> PathBuf {
    let p = Path::new(stored);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base_dir.join(p)
    }
}

/// `path` made absolute against the current directory, as a string for
/// `Settings.profiles`.
pub(crate) fn absolute_string(path: &Path) -> Result<String, ConvertError> {
    std::path::absolute(path)
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|source| ConvertError::Io {
            path: path.to_path_buf(),
            source,
        })
}

/// Decodes a rig measurement frame (flat, band or wedge) and checks it is
/// linear, `Transfer::Linear` (RAW or float TIFF), with the same step 1
/// scaling as the frames. Returns the image and the camera
/// name when the decoder reports one.
///
/// # Errors
///
/// [`ConvertError::Decode`] when it cannot be decoded and
/// [`ConvertError::Invalid`] when it is not linear.
pub fn decode_linear(path: &Path) -> Result<(Image, Option<String>), ConvertError> {
    let decoded = negg_decode::decode(path)?;
    let kind = match &decoded.transfer {
        Transfer::Linear => return Ok((decoded.image, decoded.camera)),
        Transfer::Icc(_) => "a TIFF with an ICC profile",
        Transfer::Gamma22Assumed => "an integer TIFF with no profile",
    };
    Err(ConvertError::Invalid(format!(
        "{}: rig frames must be linear (RAW or 32-bit float TIFF), not {kind}",
        path.display()
    )))
}

/// Loads a rig profile and builds its runtime correction: reads and
/// validates the TOML, decodes the flat frame named by its `[flat_field]`
/// table (relative to the profile file, linear only) and
/// calls [`negg_calib::rig_correction`].
///
/// # Errors
///
/// [`ConvertError::Profile`] when the profile cannot be read or fails
/// validation, the errors of [`decode_linear`] for the flat frame, and
/// [`ConvertError::Calib`] when the correction cannot be built (a flat of
/// the wrong size, a clipped flat, a singular crosstalk matrix).
pub fn load_rig(path: &Path) -> Result<RigCorrection, ConvertError> {
    let profile = negg_io::read_rig_profile(path).map_err(|source| ConvertError::Profile {
        path: path.to_path_buf(),
        source,
    })?;
    let flat = match &profile.flat_field {
        Some(table) => Some(decode_linear(&parent_dir(path).join(&table.frame))?.0),
        None => None,
    };
    Ok(negg_calib::rig_correction(&profile, flat.as_ref())?)
}

/// Loads a film profile and returns its film correction, or `None` when the
/// profile has no `[gamma]` table or its method is `none` (a profile that
/// gives no correction).
///
/// # Errors
///
/// [`ConvertError::Profile`] when the profile cannot be read or fails
/// validation.
pub fn load_film(path: &Path) -> Result<Option<FilmCorrection>, ConvertError> {
    let profile = negg_io::read_film_profile(path).map_err(|source| ConvertError::Profile {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(profile.correction())
}

/// Builds the roll context from `roll`: loads the profiles named in
/// `roll.shared.profiles`, resolved against `base_dir`. Returns
/// with it the warnings to report (the CLI prints each as
/// `<who>: warning: <text>`): a film profile that gives no correction, and
/// a film profile measured with a different rig than the one in use.
///
/// # Errors
///
/// The errors of [`load_rig`] and [`load_film`].
pub fn build_context(
    roll: RollSettings,
    base_dir: &Path,
) -> Result<(RollContext, Vec<String>), ConvertError> {
    let mut warnings = Vec::new();
    let rig = match &roll.shared.profiles.rig {
        Some(p) => Some(load_rig(&profile_path(base_dir, p))?),
        None => None,
    };
    let film = match &roll.shared.profiles.film {
        Some(p) => {
            let film = load_film(&profile_path(base_dir, p))?;
            if film.is_none() {
                warnings.push(format!(
                    "film profile {p} has no gamma table; no film correction is applied"
                ));
            }
            film
        }
        None => None,
    };
    if let Some(f) = &film
        && let Some(warning) = rig_mismatch(f, rig.as_ref())
    {
        warnings.push(warning);
    }
    Ok((RollContext { roll, rig, film }, warnings))
}

/// The warning to print when the film profile was measured with a
/// different rig (by name, or with one rig and without the other) than the
/// one in use, else `None`.
pub fn rig_mismatch(film: &FilmCorrection, rig: Option<&RigCorrection>) -> Option<String> {
    let in_use = rig.map(|r| r.name.as_str());
    (film.rig.as_deref() != in_use).then(|| {
        format!(
            "film profile {:?} was measured with rig {} but rig {} is in use",
            film.name,
            film.rig
                .as_deref()
                .map_or("none".into(), |r| format!("{r:?}")),
            in_use.map_or("none".into(), |r| format!("{r:?}")),
        )
    })
}

/// Decodes `path`, brings it to linear light ([`prepare_input`]) and
/// applies the rig's flat field and unmix (step 2) when `rig`
/// is given.
///
/// # Errors
///
/// [`ConvertError::Decode`], [`ConvertError::Cms`], and
/// [`ConvertError::Render`] when the rig does not fit the frame (a flat of
/// another size).
pub fn prepare_frame(
    path: &Path,
    rig: Option<&RigCorrection>,
) -> Result<PreparedInput, ConvertError> {
    let decoded = negg_decode::decode(path)?;
    let mut prepared = prepare_input(decoded)?;
    if let Some(rig) = rig {
        apply_rig_linear(&mut prepared.linear, rig)?;
    }
    Ok(prepared)
}

/// Pass 1 of a roll with each frame's outcome: the pool over the frames
/// that were analysed, and per input either its analysis or the error that
/// left it out.
#[derive(Debug)]
pub struct RollAnalysis {
    /// The pool over the analysed frames ([`negg_render::pool_roll`]).
    pub pool: RollPool,
    /// One entry per input, in input order.
    pub frames: Vec<Result<FrameAnalysis, ConvertError>>,
}

/// Pass 1 of roll mode with each frame's outcome: every input is decoded
/// in turn (one at a time, so one full-size image is held), prepared with
/// the rig ([`prepare_frame`]) and analysed with the rig density model
/// ([`analyse_frame`]); the analysed frames are then pooled with their
/// settings and the film correction ([`pool_roll`]). A frame whose
/// settings fail validation, or that fails to decode or prepare, is left
/// out of the pool with its error.
///
/// # Errors
///
/// [`ConvertError::Render`] when the pooling itself fails.
pub fn analyse_roll_frames(
    inputs: &[(PathBuf, Settings)],
    ctx: &RollContext,
) -> Result<RollAnalysis, ConvertError> {
    let density = ctx.rig.as_ref().and_then(|r| r.density.as_ref());
    let frames: Vec<Result<FrameAnalysis, ConvertError>> = inputs
        .iter()
        .map(|(path, settings)| {
            settings.validate()?;
            let prepared = prepare_frame(path, ctx.rig.as_ref())?;
            Ok(analyse_frame(
                &prepared.linear,
                density,
                &settings.geometry,
            )?)
        })
        .collect();
    let names: Vec<String> = inputs.iter().map(|(p, _)| file_name(p)).collect();
    let roll_frames: Vec<RollFrame> = frames
        .iter()
        .enumerate()
        .filter_map(|(i, a)| {
            a.as_ref().ok().map(|analysis| RollFrame {
                source: &names[i],
                analysis,
                settings: &inputs[i].1,
            })
        })
        .collect();
    let pool = pool_roll(&roll_frames, &ctx.roll, ctx.film.as_ref())?;
    Ok(RollAnalysis { pool, frames })
}

/// Pass 1 of roll mode: [`analyse_roll_frames`] keeping only the pool. A
/// frame that fails is left out of the pool.
///
/// # Errors
///
/// As [`analyse_roll_frames`].
pub fn analyse_roll(
    inputs: &[(PathBuf, Settings)],
    ctx: &RollContext,
) -> Result<RollPool, ConvertError> {
    analyse_roll_frames(inputs, ctx).map(|a| a.pool)
}

/// Pass 2 of roll mode for one frame: renders the prepared frame with its
/// resolved `settings`, the rig density model, the film correction and the
/// pooled values of `pool` filtered by the roll's `pool_*` flags
/// ([`negg_render::render_frame_with`]; the lock precedence of ADR 0017 is
/// applied there). With no rig, film or pool this renders exactly as phase 1.
///
/// # Errors
///
/// [`ConvertError::Render`] for any render error.
pub fn render_in_roll(
    prepared: &PreparedInput,
    settings: &Settings,
    ctx: &RollContext,
    pool: Option<&RollPool>,
) -> Result<FrameRender, ConvertError> {
    let pooled = pool.map(|p| p.pooled(&ctx.roll));
    let render_ctx = RenderContext {
        density: ctx.rig.as_ref().and_then(|r| r.density.as_ref()),
        film: ctx.film.as_ref(),
        pooled: pooled.as_ref(),
    };
    Ok(render_frame_with(
        &prepared.linear,
        settings,
        &render_ctx,
        prepared.camera_to_working,
    )?)
}

/// The overrides recorded in `input`'s existing sidecar, or none
/// when it has no sidecar.
///
/// # Errors
///
/// [`ConvertError::FrameSidecar`] when a sidecar exists but cannot be read
/// or parsed. The caller must then leave the frame unconverted, so that
/// the sidecar, which may hold `negg lock` values or hand-written
/// overrides, is never replaced by one without them.
pub fn read_overrides(input: &Path) -> Result<FrameOverrides, ConvertError> {
    let path = negg_io::sidecar_path(input);
    if !path.exists() {
        return Ok(FrameOverrides::default());
    }
    negg_io::read_sidecar(&path)
        .map(|sidecar| sidecar.overrides)
        .map_err(|source| ConvertError::FrameSidecar { path, source })
}

/// The overrides of a frame sidecar the caller has read already: `bytes`
/// are the contents of `path` (`None` when it does not exist). Errors as
/// [`read_overrides`].
pub(crate) fn overrides_from_bytes(
    path: &Path,
    bytes: Option<&[u8]>,
) -> Result<FrameOverrides, ConvertError> {
    let Some(bytes) = bytes else {
        return Ok(FrameOverrides::default());
    };
    serde_json::from_slice(bytes)
        .map_err(negg_io::IoError::from)
        .and_then(negg_io::migrate)
        .map(|sidecar| sidecar.overrides)
        .map_err(|source| ConvertError::FrameSidecar {
            path: path.to_path_buf(),
            source,
        })
}

/// The roll sidecar the caller has read already: `bytes` are the contents
/// of `path` (`None` when it does not exist). Errors as [`read_roll`].
pub(crate) fn roll_from_bytes(
    path: &Path,
    bytes: Option<&[u8]>,
) -> Result<Option<RollSidecar>, ConvertError> {
    let Some(bytes) = bytes else {
        return Ok(None);
    };
    serde_json::from_slice(bytes)
        .map_err(negg_io::IoError::from)
        .and_then(negg_io::migrate_roll)
        .map(Some)
        .map_err(|source| ConvertError::RollSidecar {
            path: path.to_path_buf(),
            source,
        })
}

/// Whether a roll sidecar's stored pool may be used with the roll settings
/// `effective` of this run: only when the profiles, the `pool_*` flags and
/// the shared analysis rectangle (the analysis area, else the crop) and
/// analysis buffer equal those the pool was computed with (`stored`). The
/// pooled Dmin and gamma are in the density space of the stored rig, so
/// another rig or film profile makes them meaningless; another
/// analysis area, crop or buffer changes what every frame's analysis saw
/// ([`negg_types::Geometry::same_analysis`]). Rotation
/// and flip do not touch the analysis, so they leave the pool current, as
/// does a crop that an analysis area overrides.
pub fn pool_is_current(stored: &RollSettings, effective: &RollSettings) -> bool {
    let same_area = stored
        .shared
        .geometry
        .same_analysis(&effective.shared.geometry);
    stored.shared.profiles == effective.shared.profiles
        && same_area
        && stored.pool_film_base == effective.pool_film_base
        && stored.pool_gamma == effective.pool_gamma
        && stored.pool_colour == effective.pool_colour
}

/// Reads the roll sidecar at `path` when it exists.
pub(crate) fn read_roll(path: &Path) -> Result<Option<RollSidecar>, ConvertError> {
    if !path.is_file() {
        return Ok(None);
    }
    negg_io::read_roll_sidecar(path)
        .map(Some)
        .map_err(|source| ConvertError::RollSidecar {
            path: path.to_path_buf(),
            source,
        })
}

/// Which estimates `negg lock` fixes. All `false` means all three.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LockWhich {
    /// The film base (Dmin).
    pub dmin: bool,
    /// The per-channel gamma.
    pub gamma: bool,
    /// The colour cast correction.
    pub cast: bool,
}

impl LockWhich {
    /// The selection with "none selected" read as all three.
    fn resolved(self) -> LockWhich {
        if self.dmin || self.gamma || self.cast {
            self
        } else {
            LockWhich {
                dmin: true,
                gamma: true,
                cast: true,
            }
        }
    }
}

/// `overrides` with the selected `estimates` written in as Fixed values:
/// Dmin as the film base, gamma, and the cast. Overrides that are
/// not selected are kept as they are.
pub fn lock_overrides(
    overrides: &FrameOverrides,
    estimates: &Estimates,
    which: LockWhich,
) -> FrameOverrides {
    let which = which.resolved();
    let mut locked = overrides.clone();
    if which.dmin {
        locked.film.film_base = Some(ChannelEstimate::Fixed {
            value: estimates.dmin,
        });
    }
    if which.gamma {
        locked.film.gamma = Some(ChannelEstimate::Fixed {
            value: estimates.gamma,
        });
    }
    if which.cast {
        let c = estimates.cast;
        locked.colour.cast = Some(CastEstimate::Fixed {
            slope_r: c.slope_r,
            offset_r: c.offset_r,
            slope_b: c.slope_b,
            offset_b: c.offset_b,
        });
    }
    locked
}

/// What `negg lock` did: the sidecar written and the names of the locked
/// estimates (`dmin`, `gamma`, `cast`, in that order).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Locked {
    /// The frame's sidecar, rewritten.
    pub sidecar: PathBuf,
    /// The estimates locked.
    pub locked: Vec<&'static str>,
    /// The frame's Dmax in its sidecar is pinned at the density cap:
    /// its analysed area held the film holder, so the values
    /// just locked come from that analysis, and a locked gamma keeps the
    /// frame out of the gamma pool for good. `negg lock` warns; it does
    /// not refuse, since the user may mean it.
    pub dmax_pinned: bool,
}

/// `negg lock`: writes the frame's current Dmin, gamma and cast estimates
/// from its sidecar into the sidecar's overrides as Fixed
/// ([`lock_overrides`]). The sidecar's settings are updated to match (the
/// locked values are the estimates the image was rendered with, so the
/// image stays valid), and its hash recomputed.
///
/// # Errors
///
/// [`ConvertError::Invalid`] when the sidecar cannot be read or has no
/// estimates, [`ConvertError::Render`] when the locked settings fail
/// validation, and [`ConvertError::Write`] when it cannot be written.
pub fn lock_frame(frame: &Path, which: LockWhich) -> Result<Locked, ConvertError> {
    let path = negg_io::sidecar_path(frame);
    let mut sidecar = negg_io::read_sidecar(&path)
        .map_err(|e| ConvertError::Invalid(format!("{}: {e}", path.display())))?;
    let est = sidecar.estimates.ok_or_else(|| {
        ConvertError::Invalid(format!(
            "{}: no estimates to lock; convert the frame first",
            path.display()
        ))
    })?;
    let which = which.resolved();
    let overrides = lock_overrides(&sidecar.overrides, &est, which);
    let mut locked = Vec::new();
    if which.dmin
        && let Some(v) = overrides.film.film_base
    {
        sidecar.settings.film.film_base = v;
        locked.push("dmin");
    }
    if which.gamma
        && let Some(v) = overrides.film.gamma
    {
        sidecar.settings.film.gamma = v;
        locked.push("gamma");
    }
    if which.cast
        && let Some(v) = overrides.colour.cast
    {
        sidecar.settings.colour.cast = v;
        locked.push("cast");
    }
    sidecar.overrides = overrides;
    sidecar.settings.validate()?;
    sidecar.settings_hash = sidecar.settings.content_hash();
    negg_io::write_sidecar(&path, &sidecar)?;
    Ok(Locked {
        sidecar: path,
        locked,
        dmax_pinned: negg_types::dmax_pinned(&est.dmax),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use negg_types::{CastCorrection, CastSource, DminSource, GammaSource};

    #[test]
    fn profile_paths_resolve_against_the_sidecar_directory() {
        let base = Path::new("/rolls/a");
        assert_eq!(
            profile_path(base, "rig.negg.toml"),
            Path::new("/rolls/a/rig.negg.toml")
        );
        assert_eq!(
            profile_path(base, "/p/rig.negg.toml"),
            Path::new("/p/rig.negg.toml")
        );
        assert_eq!(parent_dir(Path::new("f.dng")), Path::new("."));
        assert_eq!(parent_dir(Path::new("r/f.dng")), Path::new("r"));
        assert_eq!(file_name(Path::new("r/f.dng")), "f.dng");
    }

    #[test]
    fn a_film_profile_from_another_rig_is_warned_about() {
        let film = |rig: Option<&str>| FilmCorrection {
            name: "stock".into(),
            rig: rig.map(str::to_owned),
            gamma_ratio: [1.0; 3],
            offset_log_d: [0.0; 3],
        };
        let rig = |name: &str| RigCorrection {
            name: name.into(),
            flat: None,
            unmix: None,
            density: None,
        };
        assert_eq!(rig_mismatch(&film(None), None), None);
        assert_eq!(
            rig_mismatch(&film(Some("bench")), Some(&rig("bench"))),
            None
        );
        let w = rig_mismatch(&film(Some("bench")), Some(&rig("copy stand"))).unwrap();
        assert!(
            w.contains("\"bench\"") && w.contains("\"copy stand\""),
            "{w}"
        );
        let w = rig_mismatch(&film(Some("bench")), None).unwrap();
        assert!(w.ends_with("but rig none is in use"), "{w}");
        assert!(rig_mismatch(&film(None), Some(&rig("bench"))).is_some());
    }

    #[test]
    fn a_missing_profile_is_a_usage_error() {
        let err = load_rig(Path::new("/nonexistent/x.rig.negg.toml")).unwrap_err();
        assert!(err.is_usage_error(), "{err}");
        assert!(err.to_string().starts_with("profile "), "{err}");
        let err = load_film(Path::new("/nonexistent/x.film.negg.toml")).unwrap_err();
        assert!(err.is_usage_error(), "{err}");
    }

    fn estimates() -> Estimates {
        Estimates {
            dmin: [0.2, 0.5, 0.7],
            dmin_source: DminSource::Border,
            dmax: [1.9, 2.2, 2.4],
            gamma: [1.7, 1.6, 1.5],
            gamma_source: GammaSource::Range,
            neutral_offset: [0.0; 3],
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
        }
    }

    #[test]
    fn lock_with_nothing_selected_locks_all_three() {
        let e = estimates();
        let o = lock_overrides(&FrameOverrides::default(), &e, LockWhich::default());
        assert_eq!(
            o.film.film_base,
            Some(ChannelEstimate::Fixed { value: e.dmin })
        );
        assert_eq!(
            o.film.gamma,
            Some(ChannelEstimate::Fixed { value: e.gamma })
        );
        assert_eq!(
            o.colour.cast,
            Some(CastEstimate::Fixed {
                slope_r: 1.05,
                offset_r: -0.01,
                slope_b: 0.97,
                offset_b: 0.02,
            })
        );
    }

    #[test]
    fn lock_keeps_the_overrides_it_does_not_select() {
        let e = estimates();
        let mut before = FrameOverrides::default();
        before.film.gamma = Some(ChannelEstimate::Fixed { value: [2.0; 3] });
        let which = LockWhich {
            dmin: true,
            ..LockWhich::default()
        };
        let o = lock_overrides(&before, &e, which);
        assert_eq!(
            o.film.film_base,
            Some(ChannelEstimate::Fixed { value: e.dmin })
        );
        assert_eq!(o.film.gamma, before.film.gamma);
        assert_eq!(o.colour.cast, None);
    }

    #[test]
    fn an_analysis_area_change_makes_the_pool_stale_like_a_crop_change() {
        let rect = |x: f32| negg_types::CropRect {
            x,
            y: 0.1,
            width: 0.8,
            height: 0.8,
        };
        let stored = RollSettings::default();
        let mut area = stored.clone();
        area.shared.geometry.analysis_area = Some(rect(0.1));
        assert!(!pool_is_current(&stored, &area));
        let mut cropped = stored.clone();
        cropped.shared.geometry.crop = Some(rect(0.1));
        assert!(!pool_is_current(&stored, &cropped));
        // The same analysed rectangle as a crop or as an area is current.
        assert!(pool_is_current(&cropped, &area));
        let mut moved = area.clone();
        moved.shared.geometry.analysis_area = Some(rect(0.05));
        assert!(!pool_is_current(&area, &moved));
        let mut buffered = area.clone();
        buffered.shared.geometry.analysis_buffer = 0.1;
        assert!(!pool_is_current(&area, &buffered));
        // A crop the area overrides, a rotation or a flip keeps it current.
        let mut framed = area.clone();
        framed.shared.geometry.crop = Some(rect(0.0));
        framed.shared.geometry.rotate_degrees = 180;
        assert!(pool_is_current(&area, &framed));
    }
}
