// SPDX-License-Identifier: Apache-2.0

//! Roll mode for `negg convert` and `negg batch`, profile loading, and
//! `negg lock` (ADR 0017, R-roll, R-pool, R-lock; ADR 0018, R-paths).
//!
//! A roll is the set of batch inputs of one directory. `negg batch` renders
//! it in two passes: pass 1 decodes one frame at a time, applies the rig's
//! step 2 corrections and keeps only its analysis
//! ([`negg_render::analyse_frame`]); the frames are then pooled
//! ([`negg_render::pool_roll`]); pass 2 decodes each frame again and renders
//! it with the pooled values ([`render_in_roll`]). The roll sidecar
//! `roll.negg.json` beside the frames records the roll settings, the pool
//! and the frame list. `negg convert` uses an existing roll sidecar that
//! lists the frame, and never rewrites it.
//!
//! Profile paths in `Settings.profiles` resolve relative to the directory
//! of the sidecar holding them, which is the frames' directory; paths
//! given with `--rig` and `--film` are made absolute before they are stored
//! (R-paths).
//!
//! Owns orchestration only; the maths is in `negg-render`, `negg-film` and
//! `negg-calib`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use negg_decode::Transfer;
use negg_io::{ROLL_SIDECAR_NAME, ROLL_SIDECAR_SCHEMA_VERSION, RollSidecar};
use negg_render::{
    FrameAnalysis, FrameRender, RenderContext, RollFrame, analyse_frame, apply_rig_linear,
    pool_roll, render_frame_with,
};
use negg_types::roll::resolve;
use negg_types::settings::ChannelEstimate;
use negg_types::{
    CastEstimate, FilmCorrection, FrameOverrides, Image, PoolFrame, PoolRole, RigCorrection,
    RollPool, RollSettings, Settings,
};

use crate::cli::{BatchArgs, ConvertArgs, LockArgs, RollArgs};
use crate::convert::{
    BatchSummary, ConvertError, Converted, FrameOptions, PreparedInput, batch_inputs,
    check_targets, convert_frame_with, format_stats, load_settings, output_dir, output_path,
    prepare_input, source_name,
};

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
/// file that holds it (R-paths). Absolute paths are kept as they are.
pub fn profile_path(base_dir: &Path, stored: &str) -> PathBuf {
    let p = Path::new(stored);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base_dir.join(p)
    }
}

/// `path` made absolute against the current directory, as a string for
/// `Settings.profiles` (R-paths).
fn absolute_string(path: &Path) -> Result<String, ConvertError> {
    std::path::absolute(path)
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|source| ConvertError::Io {
            path: path.to_path_buf(),
            source,
        })
}

/// Stores `--rig` and `--film` in `settings.profiles` as absolute paths.
fn apply_profile_flags(settings: &mut Settings, args: &RollArgs) -> Result<(), ConvertError> {
    if let Some(rig) = &args.rig {
        settings.profiles.rig = Some(absolute_string(rig)?);
    }
    if let Some(film) = &args.film {
        settings.profiles.film = Some(absolute_string(film)?);
    }
    Ok(())
}

/// Decodes a rig measurement frame (flat, band or wedge) and checks it is
/// linear, `Transfer::Linear` (RAW or float TIFF), with the same step 1
/// scaling as the frames (R-rig-decode). Returns the image and the camera
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
/// table (relative to the profile file, linear only, R-rig-decode) and
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
/// gives no correction, R-film-profile).
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
/// `roll.shared.profiles`, resolved against `base_dir` (R-paths), and warns
/// on stderr, prefixed with `who`, when the film profile was measured with
/// a different rig than the one in use (R-film-profile).
///
/// # Errors
///
/// The errors of [`load_rig`] and [`load_film`].
pub fn build_context(
    roll: RollSettings,
    base_dir: &Path,
    who: &str,
) -> Result<RollContext, ConvertError> {
    let rig = match &roll.shared.profiles.rig {
        Some(p) => Some(load_rig(&profile_path(base_dir, p))?),
        None => None,
    };
    let film = match &roll.shared.profiles.film {
        Some(p) => {
            let film = load_film(&profile_path(base_dir, p))?;
            if film.is_none() {
                eprintln!(
                    "{who}: warning: film profile {p} has no gamma table; no film correction \
                     is applied"
                );
            }
            film
        }
        None => None,
    };
    if let Some(f) = &film
        && let Some(warning) = rig_mismatch(f, rig.as_ref())
    {
        eprintln!("{who}: warning: {warning}");
    }
    Ok(RollContext { roll, rig, film })
}

/// The warning to print when the film profile was measured with a
/// different rig (by name, or with one rig and without the other) than the
/// one in use (R-film-profile), else `None`.
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
/// applies the rig's flat field and unmix (step 2, R-rig-order) when `rig`
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
            Ok(analyse_frame(&prepared.linear, density))
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
/// ([`negg_render::render_frame_with`]; the precedence of R-lock is applied
/// there). With no rig, film or pool this renders exactly as phase 1.
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

/// The overrides recorded in `input`'s existing sidecar (R-lock), or none
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

/// Whether a roll sidecar's stored pool may be used with the roll settings
/// `effective` of this run: only when the profiles and the `pool_*` flags
/// equal those the pool was computed with (`stored`). The pooled Dmin and
/// gamma are in the density space of the stored rig, so another rig or
/// film profile makes them meaningless (Ruling 63).
pub fn pool_is_current(stored: &RollSettings, effective: &RollSettings) -> bool {
    stored.shared.profiles == effective.shared.profiles
        && stored.pool_film_base == effective.pool_film_base
        && stored.pool_gamma == effective.pool_gamma
        && stored.pool_colour == effective.pool_colour
}

/// Reads the roll sidecar at `path` when it exists.
fn read_roll(path: &Path) -> Result<Option<RollSidecar>, ConvertError> {
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

/// The roll settings for a run: `base` (an existing roll's settings, or
/// the defaults), with `--settings` replacing `shared`, `--space` setting
/// the output space, and `--rig` and `--film` stored as absolute paths.
fn run_roll_settings(
    base: RollSettings,
    output: &crate::cli::OutputArgs,
    roll_args: &RollArgs,
) -> Result<RollSettings, ConvertError> {
    let mut roll = base;
    if output.settings.is_some() {
        roll.shared = load_settings(output)?;
    } else if let Some(space) = output.space {
        roll.shared.colour.output_space = space.into();
    }
    apply_profile_flags(&mut roll.shared, roll_args)?;
    Ok(roll)
}

fn role_name(role: PoolRole) -> String {
    source_name(&role)
}

/// The extra `--dump-stats` lines of a frame rendered in roll mode: its
/// roles in the three pools and where its cast came from.
pub fn format_roll_stats(record: Option<&PoolFrame>, converted: &Converted) -> String {
    let mut s = String::new();
    if let Some(f) = record {
        s.push_str(&format!(
            "roll roles: dmin {}, gamma {}, cast {}\n",
            role_name(f.dmin_role),
            role_name(f.gamma_role),
            role_name(f.cast_role)
        ));
    }
    s.push_str(&format!(
        "cast source: {}\n",
        source_name(&converted.render.estimates.cast_source)
    ));
    s
}

/// Runs `negg convert`. When `--no-roll` is not given and the frame's
/// directory holds a roll sidecar listing the frame, the roll's settings
/// and pool are used (R-roll) and the roll sidecar is left as it is.
/// `--settings` replaces the shared settings for this run; the frame's
/// sidecar overrides are applied in every mode (R-lock). When `--rig` or
/// `--film` make the profiles differ from those the roll was pooled with,
/// the pool is not used: a warning says the roll must be batched again and
/// the frame is rendered with the roll settings but without the pool, and
/// its sidecar records no `roll` (Ruling 63).
///
/// # Errors
///
/// [`ConvertError::Settings`] for a bad settings file,
/// [`ConvertError::RollSidecar`] for an unreadable roll sidecar,
/// [`ConvertError::Profile`] for a bad profile,
/// [`ConvertError::FrameSidecar`] for an existing frame sidecar that
/// cannot be read (nothing is written); otherwise whatever
/// [`convert_frame_with`] returns.
pub fn run_convert(args: &ConvertArgs) -> Result<(), ConvertError> {
    let who = "negg convert";
    let input = &args.input;
    let base_dir = parent_dir(input);
    let roll_sidecar = if args.roll.no_roll {
        None
    } else {
        read_roll(&negg_io::roll_sidecar_path(&base_dir))?
            .filter(|r| r.frames.contains(&file_name(input)))
    };
    let base = match &roll_sidecar {
        Some(r) => r.settings.clone(),
        None => RollSettings {
            shared: load_settings(&args.output)?,
            ..RollSettings::default()
        },
    };
    let roll = if roll_sidecar.is_some() {
        run_roll_settings(base, &args.output, &args.roll)?
    } else {
        let mut roll = base;
        apply_profile_flags(&mut roll.shared, &args.roll)?;
        roll
    };
    let pool_current = roll_sidecar
        .as_ref()
        .is_some_and(|r| pool_is_current(&r.settings, &roll));
    if roll_sidecar.is_some() {
        let roll_path = negg_io::roll_sidecar_path(&base_dir);
        if pool_current {
            eprintln!(
                "{who}: using the roll settings and pool of {}",
                roll_path.display()
            );
        } else {
            eprintln!(
                "{who}: warning: the rig or film profile differs from the one {} was pooled \
                 with, so its pool is not used and this frame is rendered on its own; batch the \
                 roll again to pool it with the new profiles",
                roll_path.display()
            );
        }
    }
    let overrides = read_overrides(input)?;
    let ctx = build_context(roll, &base_dir, who)?;
    let settings = resolve(&ctx.roll, &overrides);
    let out_dir = output_dir(&args.output, input)?;
    let pool = roll_sidecar
        .as_ref()
        .filter(|_| pool_current)
        .and_then(|r| r.pool.as_ref());
    let opts = FrameOptions {
        ctx: Some(&ctx),
        pool,
        overrides: Some(&overrides),
        roll: pool_current.then_some(ROLL_SIDECAR_NAME),
    };
    let converted = convert_frame_with(input, &out_dir, &settings, args.output.format, &opts)?;
    println!(
        "wrote {} and {}",
        converted.output.display(),
        converted.sidecar.display()
    );
    if args.output.dump_stats {
        print!("{}", format_stats(input, &converted));
        if pool_current {
            let name = file_name(input);
            let record = pool.and_then(|p| p.frames.iter().find(|f| f.source == name));
            print!("{}", format_roll_stats(record, &converted));
        }
    }
    if args.timing {
        eprintln!("{who}: timing: {}", converted.timing);
    }
    Ok(())
}

/// One batch input after the checks that need no decode.
struct BatchFrame {
    input: PathBuf,
    output: PathBuf,
    overrides: FrameOverrides,
    settings: Settings,
}

/// Runs `negg batch`: every frame in the directory in name order. Roll
/// mode unless `--no-roll` (R-roll): the roll settings come from an
/// existing `roll.negg.json` (else the defaults), `--settings` replaces
/// their `shared` settings for the run and `--rig` and `--film` set the
/// profiles; each frame resolves the roll settings with its sidecar
/// overrides (R-lock); pass 1 analyses and pools the roll, pass 2 renders
/// each frame with the pool and writes its output and sidecar (with
/// `roll = "roll.negg.json"`); last the roll sidecar is written with the
/// pool and the frame list. A frame that fails in pass 1 is reported and
/// left out of the pool and pass 2. A frame whose existing sidecar cannot
/// be read fails before pass 1 ([`ConvertError::FrameSidecar`]) and nothing
/// of it is written, so its sidecar is kept as it is. With `--no-roll` each
/// frame is
/// rendered on its own exactly as phase 1 (its overrides still apply). A
/// failing frame prints its error to stderr and the batch continues; the
/// summary is printed last. A source whose output path an earlier source
/// in the run already produced fails with [`ConvertError::DuplicateOutput`]
/// and is skipped; a source whose sidecar would be the roll sidecar fails
/// with [`ConvertError::RollSidecarClash`].
///
/// # Errors
///
/// [`ConvertError::Settings`], [`ConvertError::NotADirectory`],
/// [`ConvertError::Io`], [`ConvertError::RollSidecar`],
/// [`ConvertError::Profile`] or a pooling error before any frame is
/// written. Per-frame failures are counted in the returned summary, not
/// returned.
pub fn run_batch(args: &BatchArgs) -> Result<BatchSummary, ConvertError> {
    let who = "negg batch";
    let inputs = batch_inputs(&args.dir)?;
    let out_dir = output_dir(&args.output, &args.dir.join("frame"))?;
    let roll_path = negg_io::roll_sidecar_path(&args.dir);
    let existing = if args.roll.no_roll {
        None
    } else {
        read_roll(&roll_path)?
    };
    let roll = if args.roll.no_roll {
        let mut roll = RollSettings {
            shared: load_settings(&args.output)?,
            ..RollSettings::default()
        };
        apply_profile_flags(&mut roll.shared, &args.roll)?;
        roll
    } else {
        let base = existing.map(|r| r.settings).unwrap_or_default();
        run_roll_settings(base, &args.output, &args.roll)?
    };
    let ctx = build_context(roll, &args.dir, who)?;

    let mut summary = BatchSummary {
        found: inputs.len(),
        converted: 0,
        failed: 0,
    };
    let fail = |summary: &mut BatchSummary, input: &Path, err: ConvertError| {
        summary.failed += 1;
        eprintln!("{who}: {}: {err}", input.display());
    };

    // Checks that need no decode: duplicate outputs, writing over the
    // source, the roll sidecar name.
    let mut produced: HashMap<PathBuf, PathBuf> = HashMap::new();
    let mut frames = Vec::new();
    for input in &inputs {
        let output = output_path(input, &out_dir, args.output.format);
        if let Some(first) = produced.get(&output) {
            let err = ConvertError::DuplicateOutput {
                output: output.clone(),
                first: first.clone(),
            };
            fail(&mut summary, input, err);
            continue;
        }
        if let Err(err) = check_targets(input, &output) {
            fail(&mut summary, input, err);
            continue;
        }
        produced.insert(output.clone(), input.clone());
        let overrides = match read_overrides(input) {
            Ok(overrides) => overrides,
            Err(err) => {
                fail(&mut summary, input, err);
                continue;
            }
        };
        let settings = resolve(&ctx.roll, &overrides);
        frames.push(BatchFrame {
            input: input.clone(),
            output,
            overrides,
            settings,
        });
    }

    let print_converted = |summary: &mut BatchSummary,
                           frame: &BatchFrame,
                           result: Result<Converted, ConvertError>,
                           record: Option<&PoolFrame>| {
        match result {
            Ok(converted) => {
                summary.converted += 1;
                debug_assert_eq!(converted.output, frame.output);
                println!(
                    "wrote {} and {}",
                    converted.output.display(),
                    converted.sidecar.display()
                );
                if args.output.dump_stats {
                    print!("{}", format_stats(&frame.input, &converted));
                    if !args.roll.no_roll {
                        print!("{}", format_roll_stats(record, &converted));
                    }
                }
            }
            Err(err) => fail(summary, &frame.input, err),
        }
    };

    if args.roll.no_roll {
        for frame in &frames {
            let opts = FrameOptions {
                ctx: Some(&ctx),
                pool: None,
                overrides: Some(&frame.overrides),
                roll: None,
            };
            let result = convert_frame_with(
                &frame.input,
                &out_dir,
                &frame.settings,
                args.output.format,
                &opts,
            );
            print_converted(&mut summary, frame, result, None);
        }
        println!("{summary}");
        return Ok(summary);
    }

    // Pass 1: analyse every frame, then pool.
    let list: Vec<(PathBuf, Settings)> = frames
        .iter()
        .map(|f| (f.input.clone(), f.settings.clone()))
        .collect();
    let analysis = analyse_roll_frames(&list, &ctx)?;
    let mut analysed = Vec::new();
    for (frame, outcome) in frames.iter().zip(analysis.frames) {
        match outcome {
            Ok(_) => analysed.push(frame),
            Err(err) => fail(&mut summary, &frame.input, err),
        }
    }
    let pool = analysis.pool;
    warn_about_pool(who, &pool, &ctx.roll);

    // Pass 2: render each analysed frame with the pool.
    for (frame, record) in analysed.iter().zip(&pool.frames) {
        debug_assert_eq!(
            file_name(&frame.input),
            record.source,
            "pass 2 pairs each analysed frame with its own pool record"
        );
        let opts = FrameOptions {
            ctx: Some(&ctx),
            pool: Some(&pool),
            overrides: Some(&frame.overrides),
            roll: Some(ROLL_SIDECAR_NAME),
        };
        let result = convert_frame_with(
            &frame.input,
            &out_dir,
            &frame.settings,
            args.output.format,
            &opts,
        );
        print_converted(&mut summary, frame, result, Some(record));
    }

    if !pool.frames.is_empty() {
        let sidecar = RollSidecar {
            schema_version: ROLL_SIDECAR_SCHEMA_VERSION,
            settings_hash: ctx.roll.content_hash(),
            settings: ctx.roll.clone(),
            frames: pool.frames.iter().map(|f| f.source.clone()).collect(),
            pool: Some(pool),
            negg_version: env!("CARGO_PKG_VERSION").to_owned(),
        };
        negg_io::write_roll_sidecar(&roll_path, &sidecar).map_err(|source| {
            ConvertError::RollSidecar {
                path: roll_path.clone(),
                source,
            }
        })?;
        println!("wrote {}", roll_path.display());
    }
    println!("{summary}");
    Ok(summary)
}

/// Reports on stderr the frames rejected from a pool and a pool with no
/// inliers (R-pool: those frames keep their own estimates).
fn warn_about_pool(who: &str, pool: &RollPool, roll: &RollSettings) {
    for f in &pool.frames {
        for (what, role) in [
            ("Dmin", f.dmin_role),
            ("gamma", f.gamma_role),
            ("cast", f.cast_role),
        ] {
            if role == PoolRole::Rejected {
                eprintln!("{who}: {}: rejected from the {what} pool", f.source);
            }
        }
    }
    let any = |pick: fn(&PoolFrame) -> PoolRole| {
        pool.frames
            .iter()
            .any(|f| matches!(pick(f), PoolRole::Inlier | PoolRole::Rejected))
    };
    if roll.pool_film_base && pool.dmin.is_none() && any(|f| f.dmin_role) {
        eprintln!(
            "{who}: warning: no frame is a Dmin inlier, so there is no pooled film base; each \
             frame keeps its own"
        );
    }
    if roll.pool_gamma && pool.gamma.is_none() && any(|f| f.gamma_role) {
        eprintln!(
            "{who}: warning: no frame is a gamma inlier, so there is no pooled gamma; each frame \
             keeps its own"
        );
    }
}

/// Runs `negg lock`: writes the frame's current Dmin, gamma and cast
/// estimates from its sidecar into the sidecar's overrides as Fixed
/// (R-lock). `--dmin`, `--gamma` and `--cast` select; none selects all.
/// The sidecar's settings are updated to match (the locked values are the
/// estimates the image was rendered with, so the image stays valid), and
/// its hash recomputed.
///
/// # Errors
///
/// [`ConvertError::Invalid`] when the sidecar cannot be read or has no
/// estimates, [`ConvertError::Render`] when the locked settings fail
/// validation, and [`ConvertError::Write`] when it cannot be written.
pub fn run_lock(args: &LockArgs) -> Result<(), ConvertError> {
    let path = negg_io::sidecar_path(&args.frame);
    let mut sidecar = negg_io::read_sidecar(&path)
        .map_err(|e| ConvertError::Invalid(format!("{}: {e}", path.display())))?;
    let est = sidecar.estimates.ok_or_else(|| {
        ConvertError::Invalid(format!(
            "{}: no estimates to lock; convert the frame first",
            path.display()
        ))
    })?;
    let all = !(args.dmin || args.gamma || args.cast);
    let mut locked = Vec::new();
    if all || args.dmin {
        let v = ChannelEstimate::Fixed { value: est.dmin };
        sidecar.overrides.film.film_base = Some(v);
        sidecar.settings.film.film_base = v;
        locked.push("dmin");
    }
    if all || args.gamma {
        let v = ChannelEstimate::Fixed { value: est.gamma };
        sidecar.overrides.film.gamma = Some(v);
        sidecar.settings.film.gamma = v;
        locked.push("gamma");
    }
    if all || args.cast {
        let c = est.cast;
        let v = CastEstimate::Fixed {
            slope_r: c.slope_r,
            offset_r: c.offset_r,
            slope_b: c.slope_b,
            offset_b: c.offset_b,
        };
        sidecar.overrides.colour.cast = Some(v);
        sidecar.settings.colour.cast = v;
        locked.push("cast");
    }
    sidecar.settings.validate()?;
    sidecar.settings_hash = sidecar.settings.content_hash();
    negg_io::write_sidecar(&path, &sidecar)?;
    println!("locked {} in {}", locked.join(", "), path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn profile_flags_are_stored_absolute() {
        let mut s = Settings::default();
        let args = RollArgs {
            rig: Some(PathBuf::from("rig.negg.toml")),
            film: Some(PathBuf::from("/abs/film.negg.toml")),
            no_roll: false,
        };
        apply_profile_flags(&mut s, &args).unwrap();
        assert!(Path::new(s.profiles.rig.as_deref().unwrap()).is_absolute());
        assert!(
            s.profiles
                .rig
                .as_deref()
                .unwrap()
                .ends_with("rig.negg.toml")
        );
        assert_eq!(s.profiles.film.as_deref(), Some("/abs/film.negg.toml"));
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
}
