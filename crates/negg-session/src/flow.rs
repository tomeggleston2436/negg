// SPDX-License-Identifier: Apache-2.0

//! The two whole flows: [`convert_one`] (`negg convert`) and [`batch_dir`]
//! (`negg batch`), with roll mode (R-roll, R-pool, R-lock, Ruling 63).
//!
//! Nothing here prints. What the CLI used to print as it went is reported
//! through a [`FlowEvent`] callback, in the same order, so that a caller
//! can print it (the CLI, with its `negg convert: ` and `negg batch: `
//! prefixes), show it or log it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use negg_io::{ROLL_SIDECAR_NAME, ROLL_SIDECAR_SCHEMA_VERSION, RollSidecar};
use negg_types::roll::resolve;
use negg_types::settings::OutputSpace;
use negg_types::{
    CropRect, FrameOverrides, PoolFrame, PoolRole, Preset, RollPool, RollSettings, Settings,
};

use crate::convert::{
    BatchSummary, Converted, Format, FrameOptions, WebOutput, batch_inputs, check_targets,
    convert_frame_noting, output_dir, planned_outputs,
};
use crate::error::ConvertError;
use crate::presets::preset_frame_overrides;
use crate::roll::{
    absolute_string, analyse_roll_frames, build_context, file_name, parent_dir, pool_is_current,
    read_overrides, read_roll,
};
use crate::session::BORDER_LOST_WARNING;
use crate::settings_file::{SettingsFile, read_settings_file_parts};
use crate::view::RollPresetScope;

/// What a conversion is asked for: the output and roll options of
/// `negg convert` and `negg batch`.
#[derive(Clone, Debug, Default)]
pub struct OutputRequest {
    /// Directory to write the output images into (`--out`); `None` writes
    /// beside the sources. Created if it does not exist.
    pub out_dir: Option<PathBuf>,
    /// Output file format (`--format`).
    pub format: Format,
    /// Settings already parsed (`--settings`); in roll mode they replace
    /// the roll's shared settings for the run, geometry included. Takes
    /// precedence over `settings_file`.
    pub settings: Option<Settings>,
    /// A settings file, sidecar or roll sidecar (`--settings`) to read with
    /// [`crate::parse_settings_file`] at the point the flow needs it, so
    /// that its errors come in the same order as before. In roll mode it
    /// replaces the roll's shared look and keeps the stored geometry unless
    /// it has a `geometry` key (Ruling 143). Used only when `settings` is
    /// `None`.
    pub settings_file: Option<PathBuf>,
    /// Output colour space override (`--space`), written to
    /// `colour.output_space`.
    pub space: Option<OutputSpace>,
    /// Crop override (`--crop`, Ruling 92), written to `geometry.crop`; in
    /// roll mode it is the shared crop.
    pub crop: Option<CropRect>,
    /// Analysis area override (`--analysis-area`, Ruling 134), written to
    /// `geometry.analysis_area`; in roll mode it is the shared analysis
    /// area.
    pub analysis_area: Option<CropRect>,
    /// A rig profile (`--rig`), stored in the settings as an absolute path.
    pub rig: Option<PathBuf>,
    /// A film profile (`--film`), stored in the settings as an absolute
    /// path.
    pub film: Option<PathBuf>,
    /// Render each frame on its own, as phase 1 did, reading and writing no
    /// roll sidecar (`--no-roll`).
    pub no_roll: bool,
    /// A tone preset (`--preset`, ADR 0034), applied after `--settings`
    /// (Ruling 7). Without roll mode its look is set on the settings with
    /// [`negg_types::PrintLook::apply_to_settings`] (frame look overrides
    /// still win, as `resolve` does). `negg convert` of a roll frame puts
    /// it in the frame's overrides, as
    /// [`crate::Session::apply_frame_preset`] does; `negg batch` in roll
    /// mode sets it on the roll's shared settings and treats frames with
    /// their own look as [`crate::Session::apply_roll_preset`] does.
    pub preset: Option<Preset>,
    /// `--preset-all-frames`: in `negg batch` roll mode, clear the look of
    /// frames with their own look so they take the preset
    /// ([`RollPresetScope::AllFrames`]) instead of keeping it
    /// ([`RollPresetScope::FramesWithoutOwnLook`]). Needs `preset`; a
    /// usage error anywhere else.
    pub preset_all_frames: bool,
    /// The web copy (`--web`, `--web-only`; ADR 0035), written to
    /// [`crate::web_output_path`] from the same render.
    pub web: WebOutput,
}

/// Sets the look of `req.preset`, if any, on `settings`.
fn apply_preset(settings: &mut Settings, req: &OutputRequest) {
    if let Some(p) = &req.preset {
        p.look.apply_to_settings(&p.name, &mut settings.print);
    }
}

/// Warns when `--preset` was set on the settings (no roll mode) and the
/// frame's own look overrides win over it, as `resolve` lets them.
fn warn_own_look_wins(
    input: &Path,
    overrides: &FrameOverrides,
    req: &OutputRequest,
    events: &mut dyn FnMut(FlowEvent),
) {
    if let Some(p) = &req.preset
        && overrides.print.has_own_look()
    {
        events(FlowEvent::Warning(format!(
            "{}: its sidecar overrides set its own look, which wins over --preset {:?}; \
             clear them, or use roll mode with --preset-all-frames",
            file_name(input),
            p.name
        )));
    }
}

/// The usage error of `--preset-all-frames` where it does not apply.
fn preset_all_frames_error(why: &str) -> ConvertError {
    ConvertError::Usage(format!("--preset-all-frames {why}"))
}

/// Something a flow reports as it goes, in the order the CLI prints it.
#[derive(Debug)]
pub enum FlowEvent {
    /// A warning (the CLI prints `<who>: warning: <text>`).
    Warning(String),
    /// `negg convert` uses the settings and pool of this roll sidecar (the
    /// CLI prints `<who>: using the roll settings and pool of <path>`).
    /// Sent exactly when the roll's pool is used.
    UsingRoll(PathBuf),
    /// A frame was converted. `pool_record` is the frame's record in the
    /// pool it was rendered with, when there is one.
    Converted {
        /// The source frame.
        input: PathBuf,
        /// What the conversion produced.
        converted: Box<Converted>,
        /// The frame's pool record (roles, pooled values).
        pool_record: Option<PoolFrame>,
    },
    /// A frame of a batch failed; the batch goes on (the CLI prints
    /// `<who>: <input>: <error>`).
    FrameFailed {
        /// The source frame.
        input: PathBuf,
        /// Why it failed.
        error: ConvertError,
    },
    /// The roll sidecar was written (the CLI prints `wrote <path>`).
    WroteRoll(PathBuf),
    /// A note from decoding and preparing a frame (a decoder warning, the
    /// untagged TIFF note), sent as soon as the frame is prepared and also
    /// kept in [`Converted::notes`]. The CLI prints
    /// `negg convert: <file name>: <note>` in both flows.
    Note {
        /// The source frame.
        input: PathBuf,
        /// The note.
        note: String,
    },
    /// A frame was rejected from one of the roll's pools (R-pool; the CLI
    /// prints `<who>: <frame>: rejected from the <pool> pool`).
    PoolRejected {
        /// The frame's file name, as the pool records it.
        frame: String,
        /// Which pool: `Dmin`, `gamma` or `cast`.
        pool: &'static str,
    },
    /// A frame's Dmax is pinned at the density cap, so it was left out of
    /// the gamma and cast pools and needs an analysis area (Ruling 133; the
    /// CLI prints a warning naming the frame).
    NeedsAnalysisArea {
        /// The frame's file name, as the pool records it.
        frame: String,
    },
}

/// The `--settings` of `req`: `settings` (whole settings, geometry
/// included), else `settings_file` read and parsed, else `None`.
fn settings_of(req: &OutputRequest) -> Result<Option<SettingsFile>, ConvertError> {
    Ok(match (&req.settings, &req.settings_file) {
        (Some(settings), _) => Some(SettingsFile {
            settings: settings.clone(),
            has_geometry: true,
        }),
        (None, Some(path)) => Some(read_settings_file_parts(path)?),
        (None, None) => None,
    })
}

/// The settings of `req` alone: `settings`, else `settings_file` read and
/// parsed, else the defaults; then `--space`, `--crop` and
/// `--analysis-area` applied
/// ([`apply_output_flags`]) so that the sidecar records the space that was
/// actually written.
fn load_settings(req: &OutputRequest) -> Result<Settings, ConvertError> {
    let mut settings = settings_of(req)?.map_or_else(Settings::default, |f| f.settings);
    apply_output_flags(&mut settings, req);
    Ok(settings)
}

/// Applies `--space` to `colour.output_space`, `--crop` to
/// `geometry.crop` (Ruling 92) and `--analysis-area` to
/// `geometry.analysis_area` (Ruling 134), after any `--settings`.
fn apply_output_flags(settings: &mut Settings, req: &OutputRequest) {
    if let Some(space) = req.space {
        settings.colour.output_space = space;
    }
    if let Some(crop) = req.crop {
        settings.geometry.crop = Some(crop);
    }
    if let Some(area) = req.analysis_area {
        settings.geometry.analysis_area = Some(area);
    }
}

/// Stores `--rig` and `--film` in `settings.profiles` as absolute paths.
fn apply_profile_flags(settings: &mut Settings, req: &OutputRequest) -> Result<(), ConvertError> {
    if let Some(rig) = &req.rig {
        settings.profiles.rig = Some(absolute_string(rig)?);
    }
    if let Some(film) = &req.film {
        settings.profiles.film = Some(absolute_string(film)?);
    }
    Ok(())
}

/// The roll settings for a run: `base` (an existing roll's settings, or
/// the defaults), with `--settings` replacing the shared look (film,
/// print, colour, profiles) and, only when the settings file has a
/// `geometry` key, the shared geometry (Ruling 143: the stored crop,
/// analysis area, buffer, rotation and flip are kept otherwise);
/// `--space` setting the output space, `--crop` the shared crop (Ruling
/// 92), `--analysis-area` the shared analysis area (Ruling 134), and
/// `--rig` and `--film` stored as absolute paths. Returns with them the
/// parts of the shared settings `--settings` changed (`film`, `print`,
/// `colour`, `profiles`, `geometry`), for the warning of a batch that
/// replaces stored roll settings.
fn run_roll_settings(
    base: RollSettings,
    req: &OutputRequest,
) -> Result<(RollSettings, Vec<String>), ConvertError> {
    let mut roll = base;
    let mut changed = Vec::new();
    if let Some(file) = settings_of(req)? {
        let mut shared = file.settings;
        if !file.has_geometry {
            shared.geometry = roll.shared.geometry;
        }
        changed = changed_parts(&roll.shared, &shared);
        roll.shared = shared;
    }
    apply_output_flags(&mut roll.shared, req);
    apply_profile_flags(&mut roll.shared, req)?;
    Ok((roll, changed))
}

/// The top-level parts of the settings (`film`, `print`, `colour`,
/// `profiles`, `geometry`) that differ between `a` and `b`.
fn changed_parts(a: &Settings, b: &Settings) -> Vec<String> {
    let (Ok(serde_json::Value::Object(a)), Ok(serde_json::Value::Object(b))) =
        (serde_json::to_value(a), serde_json::to_value(b))
    else {
        return Vec::new();
    };
    a.iter()
        .filter(|(k, v)| k.as_str() != "schema_version" && b.get(k.as_str()) != Some(*v))
        .map(|(k, _)| k.clone())
        .collect()
}

/// Builds the roll context and reports its warnings.
fn context_with_warnings(
    roll: RollSettings,
    base_dir: &Path,
    events: &mut dyn FnMut(FlowEvent),
) -> Result<crate::roll::RollContext, ConvertError> {
    let (ctx, warnings) = build_context(roll, base_dir)?;
    for warning in warnings {
        events(FlowEvent::Warning(warning));
    }
    Ok(ctx)
}

/// `negg convert` for one frame. When `req.no_roll` is not set and the
/// frame's directory holds a roll sidecar listing the frame, the roll's
/// settings and pool are used (R-roll) and the roll sidecar is left as it
/// is. `--settings` replaces the shared look for this run (and the shared
/// geometry only when it has a `geometry` key, Ruling 143), `--crop` sets
/// the shared crop and `--analysis-area` the shared analysis area; the
/// frame's sidecar overrides are applied in every mode (R-lock). When
/// `--rig` or `--film` make the profiles differ from those the roll was
/// pooled with, or `--crop`, `--analysis-area` or `--settings` change what
/// the shared analysis sees, the pool is not used: a warning says the
/// roll must be batched again and the frame is rendered with the roll
/// settings but without the pool, and its sidecar records no `roll`
/// (Ruling 63). `--preset` (Ruling 7) sets its look on a frame outside a
/// roll (or with `--no-roll`) after `--settings`, and on a roll frame in
/// its overrides, as [`crate::Session::apply_frame_preset`] does, so the
/// frame's sidecar records it; the roll sidecar is never written.
///
/// Events, in order: [`FlowEvent::UsingRoll`] or a [`FlowEvent::Warning`]
/// about the pool, the context's warnings, the frame's
/// [`FlowEvent::Note`]s and last [`FlowEvent::Converted`].
///
/// # Errors
///
/// [`ConvertError::Usage`] for `preset_all_frames`,
/// [`ConvertError::Settings`] for a bad settings file,
/// [`ConvertError::RollSidecar`] for an unreadable roll sidecar,
/// [`ConvertError::Profile`] for a bad profile,
/// [`ConvertError::FrameSidecar`] for an existing frame sidecar that
/// cannot be read (nothing is written); otherwise whatever
/// [`crate::convert_frame_with`] returns.
pub fn convert_one(
    input: &Path,
    req: &OutputRequest,
    events: &mut dyn FnMut(FlowEvent),
) -> Result<(), ConvertError> {
    if req.preset_all_frames {
        return Err(preset_all_frames_error(
            "applies only to negg batch in roll mode",
        ));
    }
    let base_dir = parent_dir(input);
    let roll_sidecar = if req.no_roll {
        None
    } else {
        read_roll(&negg_io::roll_sidecar_path(&base_dir))?
            .filter(|r| r.frames.contains(&file_name(input)))
    };
    let base = match &roll_sidecar {
        Some(r) => r.settings.clone(),
        None => RollSettings {
            shared: load_settings(req)?,
            ..RollSettings::default()
        },
    };
    let roll = if roll_sidecar.is_some() {
        run_roll_settings(base, req)?.0
    } else {
        let mut roll = base;
        apply_profile_flags(&mut roll.shared, req)?;
        apply_preset(&mut roll.shared, req);
        roll
    };
    let pool_current = roll_sidecar
        .as_ref()
        .is_some_and(|r| pool_is_current(&r.settings, &roll));
    if roll_sidecar.is_some() {
        let roll_path = negg_io::roll_sidecar_path(&base_dir);
        if pool_current {
            events(FlowEvent::UsingRoll(roll_path));
        } else {
            events(FlowEvent::Warning(format!(
                "the rig or film profile or the crop or analysis area differs from the one {} was \
                 pooled with, so its pool is not used and this frame is rendered on its own; batch \
                 the roll again to pool it with the new profiles, crop or analysis area",
                roll_path.display()
            )));
        }
    }
    let mut overrides = read_overrides(input)?;
    if roll_sidecar.is_some()
        && let Some(p) = &req.preset
    {
        // Ruling 7: a roll frame takes the preset as its own look, exactly
        // as `Session::apply_frame_preset` gives it.
        p.look.apply_to_overrides(&p.name, &mut overrides.print);
    } else {
        warn_own_look_wins(input, &overrides, req, events);
    }
    let ctx = context_with_warnings(roll, &base_dir, events)?;
    let settings = resolve(&ctx.roll, &overrides);
    let out_dir = output_dir(req.out_dir.as_deref(), input)?;
    let pool = roll_sidecar
        .as_ref()
        .filter(|_| pool_current)
        .and_then(|r| r.pool.as_ref());
    let opts = FrameOptions {
        ctx: Some(&ctx),
        pool,
        overrides: Some(&overrides),
        roll: pool_current.then_some(ROLL_SIDECAR_NAME),
        web: req.web,
    };
    let converted =
        convert_frame_noting(input, &out_dir, &settings, req.format, &opts, &mut |n| {
            events(FlowEvent::Note {
                input: input.to_path_buf(),
                note: n.to_owned(),
            })
        })?;
    let pool_record = if pool_current {
        let name = file_name(input);
        pool.and_then(|p| p.frames.iter().find(|f| f.source == name))
            .cloned()
    } else {
        None
    };
    events(FlowEvent::Converted {
        input: input.to_path_buf(),
        converted: Box::new(converted),
        pool_record,
    });
    Ok(())
}

/// One batch input after the checks that need no decode.
struct BatchFrame {
    input: PathBuf,
    /// The image reported as [`Converted::output`]: the original, or the
    /// web copy with [`WebOutput::Only`].
    output: PathBuf,
    overrides: FrameOverrides,
    settings: Settings,
}

/// `negg batch`: every frame in `dir` in name order. Roll mode unless
/// `req.no_roll` (R-roll): the roll settings come from an existing
/// `roll.negg.json` (else the defaults), `--settings` replaces their
/// shared look and keeps their geometry unless the settings file has a
/// `geometry` key (Ruling 143; a warning names what it replaced), and
/// `--rig` and `--film` set the profiles; when an analysis area or crop
/// change takes the roll's Dmin pool off the clear film edge, a warning
/// says so (Ruling 138); each frame resolves the roll settings with its sidecar
/// overrides (R-lock); pass 1 analyses and pools the roll, pass 2 renders
/// each frame with the pool and writes its output and sidecar (with
/// `roll = "roll.negg.json"`); last the roll sidecar is written with the
/// pool and the frame list. A frame that fails in pass 1 is reported and
/// left out of the pool and pass 2. A frame whose existing sidecar cannot
/// be read fails before pass 1 ([`ConvertError::FrameSidecar`]) and nothing
/// of it is written, so its sidecar is kept as it is. With `no_roll` each
/// frame is rendered on its own exactly as phase 1 (its overrides still
/// apply). A failing frame is reported with [`FlowEvent::FrameFailed`] and
/// the batch continues. A source whose output path (or web copy path,
/// with `req.web`) an earlier source in the run already produced fails
/// with [`ConvertError::DuplicateOutput`] and is skipped; a source whose sidecar would be the roll sidecar fails
/// with [`ConvertError::RollSidecarClash`].
///
/// `--preset` (Ruling 7) applies after `--settings`: with `no_roll` to the
/// settings; in roll mode to the roll's shared settings (stored in the roll
/// sidecar), with each frame's overrides passed through the rule
/// [`crate::Session::apply_roll_preset`] uses
/// ([`RollPresetScope::FramesWithoutOwnLook`], or
/// [`RollPresetScope::AllFrames`] with `preset_all_frames`) and written to
/// its sidecar.
///
/// # Errors
///
/// [`ConvertError::Usage`] for `preset_all_frames` without `preset` or
/// with `no_roll`, before anything is read;
/// [`ConvertError::Settings`], [`ConvertError::NotADirectory`],
/// [`ConvertError::Io`], [`ConvertError::RollSidecar`],
/// [`ConvertError::Profile`] or a pooling error before any frame is
/// written, and [`ConvertError::RollSidecar`] when the roll sidecar cannot
/// be written at the end. Per-frame failures are counted in the returned
/// summary, not returned.
pub fn batch_dir(
    dir: &Path,
    req: &OutputRequest,
    events: &mut dyn FnMut(FlowEvent),
) -> Result<BatchSummary, ConvertError> {
    if req.preset_all_frames {
        if req.preset.is_none() {
            return Err(preset_all_frames_error("needs --preset"));
        }
        if req.no_roll {
            return Err(preset_all_frames_error(
                "applies only in roll mode, not with --no-roll",
            ));
        }
    }
    let inputs = batch_inputs(dir)?;
    let out_dir = output_dir(req.out_dir.as_deref(), &dir.join("frame"))?;
    let roll_path = negg_io::roll_sidecar_path(dir);
    let existing = if req.no_roll {
        None
    } else {
        read_roll(&roll_path)?
    };
    // The stored roll's analysis rectangle and whether its Dmin pool was
    // on the clear film edge (Ruling 138).
    let stored = existing.as_ref().map(|r| {
        (
            r.settings.shared.geometry,
            r.pool.as_ref().is_some_and(RollPool::dmin_from_border),
        )
    });
    let mut roll = if req.no_roll {
        let mut roll = RollSettings {
            shared: load_settings(req)?,
            ..RollSettings::default()
        };
        apply_profile_flags(&mut roll.shared, req)?;
        roll
    } else {
        let had_roll = existing.is_some();
        let base = existing.map(|r| r.settings).unwrap_or_default();
        let (roll, changed) = run_roll_settings(base, req)?;
        if had_roll && !changed.is_empty() {
            events(FlowEvent::Warning(format!(
                "--settings replaces the {} settings stored in {}; the roll is stored with the new \
                 ones (the stored geometry is kept unless the settings file has a geometry key)",
                changed.join(", "),
                roll_path.display()
            )));
        }
        roll
    };
    // Ruling 7: the preset is the look of the settings (roll mode: the
    // roll's), and in roll mode each frame with its own look is treated as
    // `Session::apply_roll_preset` treats it, against the print before the
    // preset.
    let frame_preset = match &req.preset {
        Some(_) if !req.no_roll => Some((
            roll.shared.print.clone(),
            if req.preset_all_frames {
                RollPresetScope::AllFrames
            } else {
                RollPresetScope::FramesWithoutOwnLook
            },
        )),
        _ => None,
    };
    apply_preset(&mut roll.shared, req);
    let ctx = context_with_warnings(roll, dir, events)?;

    let mut summary = BatchSummary {
        found: inputs.len(),
        converted: 0,
        failed: 0,
    };
    let fail = |summary: &mut BatchSummary,
                events: &mut dyn FnMut(FlowEvent),
                input: &Path,
                error: ConvertError| {
        summary.failed += 1;
        events(FlowEvent::FrameFailed {
            input: input.to_path_buf(),
            error,
        });
    };

    // Checks that need no decode: duplicate outputs (web copies included),
    // writing over the source, the roll sidecar name.
    let mut produced: HashMap<PathBuf, PathBuf> = HashMap::new();
    let mut frames = Vec::new();
    for input in &inputs {
        let outputs = planned_outputs(input, &out_dir, req.format, req.web);
        if let Some((output, first)) = outputs
            .iter()
            .find_map(|o| produced.get(o).map(|first| (o, first)))
        {
            let err = ConvertError::DuplicateOutput {
                output: output.clone(),
                first: first.clone(),
            };
            fail(&mut summary, events, input, err);
            continue;
        }
        if let Err(err) = outputs.iter().try_for_each(|o| check_targets(input, o)) {
            fail(&mut summary, events, input, err);
            continue;
        }
        for o in &outputs {
            produced.insert(o.clone(), input.clone());
        }
        let output = outputs[0].clone();
        let overrides = match read_overrides(input) {
            Ok(overrides) => match &frame_preset {
                Some((old_print, scope)) => preset_frame_overrides(old_print, &overrides, *scope),
                None => {
                    warn_own_look_wins(input, &overrides, req, events);
                    overrides
                }
            },
            Err(err) => {
                fail(&mut summary, events, input, err);
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

    let convert = |summary: &mut BatchSummary,
                   events: &mut dyn FnMut(FlowEvent),
                   frame: &BatchFrame,
                   opts: &FrameOptions,
                   record: Option<&PoolFrame>| {
        let result = convert_frame_noting(
            &frame.input,
            &out_dir,
            &frame.settings,
            req.format,
            opts,
            &mut |n| {
                events(FlowEvent::Note {
                    input: frame.input.clone(),
                    note: n.to_owned(),
                })
            },
        );
        match result {
            Ok(converted) => {
                summary.converted += 1;
                debug_assert_eq!(converted.output, frame.output);
                events(FlowEvent::Converted {
                    input: frame.input.clone(),
                    converted: Box::new(converted),
                    pool_record: record.cloned(),
                });
            }
            Err(err) => fail(summary, events, &frame.input, err),
        }
    };

    if req.no_roll {
        for frame in &frames {
            let opts = FrameOptions {
                ctx: Some(&ctx),
                pool: None,
                overrides: Some(&frame.overrides),
                roll: None,
                web: req.web,
            };
            convert(&mut summary, events, frame, &opts, None);
        }
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
            Err(err) => fail(&mut summary, events, &frame.input, err),
        }
    }
    let pool = analysis.pool;
    report_pool(&pool, &ctx.roll, events);
    if let Some((geometry, true)) = stored
        && !geometry.same_analysis(&ctx.roll.shared.geometry)
        && !pool.dmin_from_border()
    {
        events(FlowEvent::Warning(BORDER_LOST_WARNING.to_owned()));
    }

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
            web: req.web,
        };
        convert(&mut summary, events, frame, &opts, Some(record));
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
        events(FlowEvent::WroteRoll(roll_path));
    }
    Ok(summary)
}

/// Reports the frames rejected from a pool, the frames that need an
/// analysis area (Ruling 133) and a pool with no inliers (R-pool: those
/// frames keep their own estimates).
fn report_pool(pool: &RollPool, roll: &RollSettings, events: &mut dyn FnMut(FlowEvent)) {
    for f in &pool.frames {
        if f.needs_analysis_area {
            events(FlowEvent::NeedsAnalysisArea {
                frame: f.source.clone(),
            });
        }
        for (what, role) in [
            ("Dmin", f.dmin_role),
            ("gamma", f.gamma_role),
            ("cast", f.cast_role),
        ] {
            if role == PoolRole::Rejected {
                events(FlowEvent::PoolRejected {
                    frame: f.source.clone(),
                    pool: what,
                });
            }
        }
    }
    let any = |pick: fn(&PoolFrame) -> PoolRole| {
        pool.frames
            .iter()
            .any(|f| matches!(pick(f), PoolRole::Inlier | PoolRole::Rejected))
    };
    if roll.pool_film_base && pool.dmin.is_none() && any(|f| f.dmin_role) {
        events(FlowEvent::Warning(
            "no frame is a Dmin inlier, so there is no pooled film base; each frame keeps its own"
                .into(),
        ));
    }
    if roll.pool_gamma && pool.gamma.is_none() && any(|f| f.gamma_role) {
        events(FlowEvent::Warning(
            "no frame is a gamma inlier, so there is no pooled gamma; each frame keeps its own"
                .into(),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_flags_are_stored_absolute() {
        let mut s = Settings::default();
        let req = OutputRequest {
            rig: Some(PathBuf::from("rig.negg.toml")),
            film: Some(PathBuf::from("/abs/film.negg.toml")),
            ..OutputRequest::default()
        };
        apply_profile_flags(&mut s, &req).unwrap();
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
    fn parsed_settings_take_precedence_over_a_settings_file() {
        let mut parsed = Settings::default();
        parsed.print.grade = 70.0;
        let req = OutputRequest {
            settings: Some(parsed.clone()),
            settings_file: Some(PathBuf::from("/nonexistent/s.json")),
            space: Some(OutputSpace::Rec2020),
            ..OutputRequest::default()
        };
        let s = load_settings(&req).unwrap();
        assert_eq!(s.print.grade, 70.0);
        assert_eq!(s.colour.output_space, OutputSpace::Rec2020);
        let req = OutputRequest {
            settings: None,
            ..req
        };
        assert!(load_settings(&req).unwrap_err().is_usage_error());
    }

    /// A scratch directory removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let dir =
                std::env::temp_dir().join(format!("negg-session-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A synthetic negative as an untagged 16-bit TIFF: a clear border of
    /// density `dmin` around a ramp of density up to `dmin + 1.6`, stored
    /// as gamma 2.2 encoded transmittance.
    fn write_negative(path: &Path, dmin: [f32; 3]) {
        let (w, h, border) = (96usize, 64usize, 8usize);
        let mut data = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                let inside = x >= border && x < w - border && y >= border && y < h - border;
                let t = if inside {
                    let ramp = (x - border) as f32 / (w - 2 * border) as f32;
                    let texture = if (x / 3 + y / 3) % 2 == 0 { 0.0 } else { 0.05 };
                    (ramp * 0.9 + texture).min(1.0)
                } else {
                    0.0
                };
                for d in dmin {
                    let transmittance = 10f32.powf(-(d + 1.6 * t));
                    data.push((transmittance.powf(1.0 / 2.2) * 65535.0).round() as u16);
                }
            }
        }
        negg_io::write_tiff16(path, w, h, &data, &negg_io::TiffOptions::default()).unwrap();
    }

    /// Runs a flow, collecting its events.
    fn collect<T>(
        run: impl FnOnce(&mut dyn FnMut(FlowEvent)) -> Result<T, ConvertError>,
    ) -> (Result<T, ConvertError>, Vec<FlowEvent>) {
        let mut events = Vec::new();
        let result = run(&mut |e| events.push(e));
        (result, events)
    }

    #[test]
    fn convert_one_reports_the_notes_then_the_conversion() {
        let dir = Scratch::new("one");
        let input = dir.0.join("f.tif");
        write_negative(&input, [0.25, 0.55, 0.75]);
        let req = OutputRequest::default();
        let (result, mut events) = collect(|ev| convert_one(&input, &req, ev));
        result.unwrap();
        let Some(FlowEvent::Converted {
            converted,
            pool_record,
            ..
        }) = events.pop()
        else {
            panic!("the last event is not the conversion");
        };
        assert!(pool_record.is_none());
        let notes: Vec<String> = events
            .into_iter()
            .map(|e| match e {
                FlowEvent::Note { input: noted, note } if noted == input => note,
                _ => panic!("only notes come before the conversion"),
            })
            .collect();
        // The untagged TIFF gives the decoder's warning and the matrix note.
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert_eq!(notes, converted.notes);
        assert_eq!(converted.output, dir.0.join("f.negg.tif"));
        assert!(converted.output.is_file() && converted.sidecar.is_file());
    }

    #[test]
    fn batch_then_convert_and_lock_use_the_roll() {
        let dir = Scratch::new("roll");
        write_negative(&dir.0.join("a.tif"), [0.25, 0.55, 0.75]);
        write_negative(&dir.0.join("b.tif"), [0.26, 0.56, 0.76]);
        let req = OutputRequest {
            format: Format::Jpeg,
            ..OutputRequest::default()
        };
        let (summary, events) = collect(|ev| batch_dir(&dir.0, &req, ev));
        let summary = summary.unwrap();
        assert_eq!(
            (summary.found, summary.converted, summary.failed),
            (2, 2, 0)
        );
        let converted: Vec<&FlowEvent> = events
            .iter()
            .filter(|e| matches!(e, FlowEvent::Converted { .. }))
            .collect();
        assert_eq!(converted.len(), 2);
        for e in &converted {
            let FlowEvent::Converted { pool_record, .. } = e else {
                unreachable!()
            };
            assert!(pool_record.is_some());
        }
        let roll_path = negg_io::roll_sidecar_path(&dir.0);
        assert!(matches!(events.last(), Some(FlowEvent::WroteRoll(p)) if *p == roll_path));
        assert!(dir.0.join("a.negg.jpg").is_file());

        // `negg convert` on a frame of the roll uses its pool.
        let input = dir.0.join("a.tif");
        let (result, events) = collect(|ev| convert_one(&input, &req, ev));
        result.unwrap();
        assert!(matches!(&events[0], FlowEvent::UsingRoll(p) if *p == roll_path));
        assert!(matches!(
            events.last(),
            Some(FlowEvent::Converted {
                pool_record: Some(_),
                ..
            })
        ));

        // Then lock its estimates.
        let locked = crate::roll::lock_frame(&input, crate::roll::LockWhich::default()).unwrap();
        assert_eq!(locked.locked, ["dmin", "gamma", "cast"]);
        let sidecar = negg_io::read_sidecar(&locked.sidecar).unwrap();
        assert!(sidecar.overrides.film.film_base.is_some());
        assert_eq!(sidecar.settings_hash, sidecar.settings.content_hash());
    }

    #[test]
    fn batch_duplicate_outputs_cover_the_web_copies() {
        let dir = Scratch::new("web-dup");
        write_negative(&dir.0.join("a.tif"), [0.25, 0.55, 0.75]);
        std::fs::copy(dir.0.join("a.tif"), dir.0.join("a.tiff")).unwrap();
        let req = OutputRequest {
            no_roll: true,
            web: WebOutput::Only,
            ..OutputRequest::default()
        };
        let (summary, events) = collect(|ev| batch_dir(&dir.0, &req, ev));
        let summary = summary.unwrap();
        assert_eq!(
            (summary.found, summary.converted, summary.failed),
            (2, 1, 1)
        );
        let web = dir.0.join("web").join("a.negg.jpg");
        assert!(events.iter().any(|e| matches!(
            e,
            FlowEvent::FrameFailed {
                error: ConvertError::DuplicateOutput { output, .. },
                ..
            } if *output == web
        )));
        assert!(web.is_file());
        assert!(!dir.0.join("a.negg.tif").exists());
    }

    #[test]
    fn batch_of_a_file_is_a_usage_error_before_any_event() {
        let dir = Scratch::new("notdir");
        let file = dir.0.join("x.tif");
        std::fs::write(&file, b"x").unwrap();
        let (result, events) = collect(|ev| batch_dir(&file, &OutputRequest::default(), ev));
        let err = result.unwrap_err();
        assert!(matches!(err, ConvertError::NotADirectory(_)), "{err:?}");
        assert!(events.is_empty());
    }
}
