// SPDX-License-Identifier: Apache-2.0

//! `negg convert` and `negg batch`: decode, render, export and write the
//! sidecar for one frame or for every frame in a directory.
//!
//! This module orchestrates and does I/O only. The maths is in the engine
//! crates: linearisation and the camera matrix in `negg-cms`, the
//! analysis and render in `negg-render` (one pipeline, ground rule 3),
//! the export transform in `negg-cms` and the writers in `negg-io`. The
//! pixels stay `f32` until [`negg_cms::OutputTransform`] quantises them
//! once for the writer (ground rule 4).
//!
//! The input is brought to linear light by its [`Transfer`]:
//!
//! - RAW (`Linear`): linear camera RGB. The camera matrix recorded at
//!   decode is turned into camera-to-Rec.2020 with
//!   [`negg_cms::camera_to_rec2020`], white-normalised for the illuminant
//!   the matrix was measured under (`Decoded::matrix_white`, ADR 0012),
//!   and applied by step 7 after inversion. Without a matrix the output
//!   stays in camera RGB, with a warning.
//! - TIFF with an ICC profile (`Icc`): linearised per channel through
//!   that profile's own TRC with [`negg_cms::icc_linearise_native`], so
//!   the scan stays in its own primaries through steps 3 to 6; the step 7
//!   matrix out of those primaries comes from the profile's colorants
//!   (ADR 0012). Only matrix/TRC profiles are accepted.
//! - Integer TIFF with no profile (`Gamma22Assumed`): undone with
//!   [`negg_cms::gamma22_to_linear`] and the decoder's warning is printed.
//!   An untagged TIFF carries no camera matrix, so step 7 is skipped and
//!   the output is in whatever space the scan was in.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use negg_cms::matrix::D65_XYZ;
use negg_cms::{CmsError, Intent};
use negg_decode::{DecodeError, Decoded, Transfer};
use negg_io::{IoError, JpegOptions, Sidecar, TiffOptions};
use negg_render::{FrameRender, render_frame};
use negg_types::settings::{OutputSpace, SETTINGS_SCHEMA_VERSION};
use negg_types::{FrameOverrides, Image, RollPool, Settings};

use crate::cli::{Format, OutputArgs};
use crate::roll::{RollContext, prepare_frame, render_in_roll};

/// File extensions (lower case, no dot) that `negg batch` converts: the
/// RAW formats rawler reads plus TIFF.
const BATCH_EXTENSIONS: [&str; 21] = [
    "dng", "cr2", "cr3", "nef", "arw", "raf", "orf", "rw2", "pef", "3fr", "iiq", "srw", "crw",
    "x3f", "dcr", "mrw", "erf", "mef", "mos", "tif", "tiff",
];

/// The infix of every file negg writes: the sidecar `<stem>.negg.json` and
/// the outputs `<stem>.negg.tif` and `<stem>.negg.jpg`. `negg batch` skips
/// any file whose name contains it, so that a source is never overwritten
/// and a previous output is never converted again.
const NEGG_INFIX: &str = ".negg.";

/// Why a settings file could not be used.
#[derive(Debug)]
pub enum SettingsError {
    /// The file could not be read.
    Io(std::io::Error),
    /// The file is not JSON for the `Settings` schema.
    Json(serde_json::Error),
    /// The file has a top-level `settings` key, so it was read as a
    /// sidecar, and [`negg_io::migrate`] rejected it.
    Sidecar(IoError),
    /// The file is a JSON object with none of the `Settings` keys and no
    /// `settings` key, so it is neither a settings file nor a sidecar.
    Unrecognised,
    /// The settings failed [`Settings::validate`].
    Invalid(negg_types::Error),
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SettingsError::Io(e) => write!(f, "cannot read: {e}"),
            SettingsError::Json(e) => write!(f, "{e}"),
            SettingsError::Sidecar(e) => write!(f, "read as a sidecar: {e}"),
            SettingsError::Unrecognised => write!(
                f,
                "not a settings file or a sidecar: no top-level {} or settings key",
                settings_keys().join(", ")
            ),
            SettingsError::Invalid(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SettingsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SettingsError::Io(e) => Some(e),
            SettingsError::Json(e) => Some(e),
            SettingsError::Sidecar(e) => Some(e),
            SettingsError::Unrecognised => None,
            SettingsError::Invalid(e) => Some(e),
        }
    }
}

/// The top-level keys of the `Settings` schema, read off the serialised
/// defaults so the list cannot drift from the struct.
fn settings_keys() -> Vec<String> {
    match serde_json::to_value(Settings::default()) {
        Ok(serde_json::Value::Object(map)) => map.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

/// Parses the text of a `--settings` file (Ruling 33): a JSON object with
/// a top-level `settings` key is a sidecar, loaded through
/// [`negg_io::migrate`], and its settings are used; an empty object, or an
/// object with at least one top-level `Settings` key, is a settings file
/// (omitted fields take their defaults, so `{}` is the defaults); anything
/// else is rejected. A settings file of schema version 1 is migrated to
/// the current version (Ruling 51). The result is validated.
///
/// # Errors
///
/// [`SettingsError::Json`] when the text is not JSON or does not fit the
/// `Settings` schema (including a value that is not an object),
/// [`SettingsError::Sidecar`] when a sidecar does not migrate,
/// [`SettingsError::Unrecognised`] for a non-empty object with no
/// recognised key,
/// and [`SettingsError::Invalid`] when the settings fail validation.
pub fn parse_settings(text: &str) -> Result<Settings, SettingsError> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(SettingsError::Json)?;
    let mut settings: Settings = match value.as_object() {
        Some(map) if map.contains_key("settings") => {
            negg_io::migrate(value)
                .map_err(SettingsError::Sidecar)?
                .settings
        }
        Some(map) if map.is_empty() || settings_keys().iter().any(|k| map.contains_key(k)) => {
            serde_json::from_value(value).map_err(SettingsError::Json)?
        }
        Some(_) => return Err(SettingsError::Unrecognised),
        None => serde_json::from_value(value).map_err(SettingsError::Json)?,
    };
    // A v1 settings file is migrated to the current schema (Ruling 51,
    // R-schema): v2 only adds fields, which take their defaults, so the
    // version number is the one thing to change. A newer version is kept
    // as it is.
    if settings.schema_version < SETTINGS_SCHEMA_VERSION {
        settings.schema_version = SETTINGS_SCHEMA_VERSION;
    }
    settings.validate().map_err(SettingsError::Invalid)?;
    Ok(settings)
}

/// Why a conversion failed.
#[derive(Debug)]
pub enum ConvertError {
    /// The `--settings` file could not be used (a usage error).
    Settings {
        /// The settings file.
        path: PathBuf,
        /// What went wrong.
        source: SettingsError,
    },
    /// `negg batch` was given something that is not a directory (a usage
    /// error).
    NotADirectory(PathBuf),
    /// The input could not be decoded.
    Decode(DecodeError),
    /// Linearisation, the camera matrix or the export transform failed.
    Cms(CmsError),
    /// The render failed (invalid settings for this frame, or an empty
    /// image).
    Render(negg_types::Error),
    /// The output image or the sidecar could not be written.
    Write(IoError),
    /// A directory could not be created or read.
    Io {
        /// The path concerned.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
    /// The output image or the sidecar would be written over the input.
    /// Refused before anything is decoded or written.
    WouldOverwriteSource {
        /// The input, which is left untouched.
        input: PathBuf,
        /// The output or sidecar path that resolves to the input.
        target: PathBuf,
    },
    /// In `negg batch`, an earlier source in the same run already wrote
    /// this output path (for example `a.tif` and `a.tiff`), so this source
    /// is skipped rather than overwrite it.
    DuplicateOutput {
        /// The output path both sources resolve to.
        output: PathBuf,
        /// The source that produced it first.
        first: PathBuf,
    },
    /// The frame's sidecar path would be the roll sidecar
    /// (`roll.negg.json`, R-roll), so the frame is refused.
    RollSidecarClash(PathBuf),
    /// A rig or film profile, or a calibration CSV, could not be read (a
    /// usage error).
    Profile {
        /// The file concerned.
        path: PathBuf,
        /// What went wrong.
        source: IoError,
    },
    /// The roll sidecar could not be read or written.
    RollSidecar {
        /// The roll sidecar.
        path: PathBuf,
        /// What went wrong.
        source: IoError,
    },
    /// The frame's existing sidecar could not be read or parsed. The frame
    /// is not rendered, so the sidecar (and any locks or overrides in it)
    /// and the output are left untouched (R-lock).
    FrameSidecar {
        /// The frame's sidecar.
        path: PathBuf,
        /// What went wrong.
        source: IoError,
    },
    /// A rig measurement failed (negg-calib).
    Calib(negg_calib::CalibError),
    /// The command line is inconsistent (a usage error), for example band
    /// frames given without all three.
    Usage(String),
    /// A measurement or lock could not be made from the data, for example a
    /// patch with too few grid cells or a frame with no estimates.
    Invalid(String),
}

impl ConvertError {
    /// Whether the error is in the command line or the settings file
    /// rather than in the conversion; the binary maps these to the usage
    /// exit status.
    pub fn is_usage_error(&self) -> bool {
        matches!(
            self,
            ConvertError::Settings { .. }
                | ConvertError::NotADirectory(_)
                | ConvertError::Profile { .. }
                | ConvertError::Usage(_)
        )
    }
}

impl fmt::Display for ConvertError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConvertError::Settings { path, source } => {
                write!(f, "settings {}: {source}", path.display())
            }
            ConvertError::NotADirectory(path) => {
                write!(f, "{} is not a directory", path.display())
            }
            ConvertError::Decode(e) => write!(f, "decode: {e}"),
            ConvertError::Cms(e) => write!(f, "colour management: {e}"),
            ConvertError::Render(e) => write!(f, "render: {e}"),
            ConvertError::Write(e) => write!(f, "write: {e}"),
            ConvertError::Io { path, source } => write!(f, "{}: {source}", path.display()),
            ConvertError::WouldOverwriteSource { input, target } => write!(
                f,
                "refusing to write {} over the source {}",
                target.display(),
                input.display()
            ),
            ConvertError::DuplicateOutput { output, first } => write!(
                f,
                "{}: output path already produced by {} in this run",
                output.display(),
                first.display()
            ),
            ConvertError::RollSidecarClash(input) => write!(
                f,
                "refusing {}: its sidecar would be the roll sidecar {}",
                input.display(),
                negg_io::ROLL_SIDECAR_NAME
            ),
            ConvertError::Profile { path, source } => {
                write!(f, "profile {}: {source}", path.display())
            }
            ConvertError::RollSidecar { path, source } => {
                write!(f, "roll sidecar {}: {source}", path.display())
            }
            ConvertError::FrameSidecar { path, source } => write!(
                f,
                "cannot read the existing sidecar {} ({source}); the frame is not converted \
                 and the sidecar is left as it is: fix or remove it",
                path.display()
            ),
            ConvertError::Calib(e) => write!(f, "calibration: {e}"),
            ConvertError::Usage(m) | ConvertError::Invalid(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for ConvertError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConvertError::Settings { source, .. } => Some(source),
            ConvertError::NotADirectory(_) => None,
            ConvertError::Decode(e) => Some(e),
            ConvertError::Cms(e) => Some(e),
            ConvertError::Render(e) => Some(e),
            ConvertError::Write(e) => Some(e),
            ConvertError::Io { source, .. } => Some(source),
            ConvertError::Profile { source, .. }
            | ConvertError::RollSidecar { source, .. }
            | ConvertError::FrameSidecar { source, .. } => Some(source),
            ConvertError::Calib(e) => Some(e),
            ConvertError::WouldOverwriteSource { .. }
            | ConvertError::DuplicateOutput { .. }
            | ConvertError::RollSidecarClash(_)
            | ConvertError::Usage(_)
            | ConvertError::Invalid(_) => None,
        }
    }
}

impl From<DecodeError> for ConvertError {
    fn from(e: DecodeError) -> Self {
        ConvertError::Decode(e)
    }
}

impl From<CmsError> for ConvertError {
    fn from(e: CmsError) -> Self {
        ConvertError::Cms(e)
    }
}

impl From<negg_types::Error> for ConvertError {
    fn from(e: negg_types::Error) -> Self {
        ConvertError::Render(e)
    }
}

impl From<negg_calib::CalibError> for ConvertError {
    fn from(e: negg_calib::CalibError) -> Self {
        ConvertError::Calib(e)
    }
}

impl From<IoError> for ConvertError {
    fn from(e: IoError) -> Self {
        ConvertError::Write(e)
    }
}

/// A decoded input brought to linear light, ready for
/// [`negg_render::render_frame`].
#[derive(Debug)]
pub struct PreparedInput {
    /// Linear camera RGB (RAW, untagged TIFF) or linear RGB in the
    /// primaries of the embedded ICC profile (TIFF with a profile).
    pub linear: Image,
    /// The step 7 camera-to-Rec.2020 matrix when the input has one.
    pub camera_to_working: Option<[[f32; 3]; 3]>,
    /// The decoder's warnings plus any note about the matrix, for stderr.
    pub notes: Vec<String>,
}

/// Brings a [`Decoded`] image to linear light per its [`Transfer`] (see
/// the module documentation) and builds the step 7 matrix when the input
/// carries one. The encoded image is dropped once linearised, so one
/// full-size copy is held on return.
///
/// # Errors
///
/// [`CmsError`] when the embedded ICC profile cannot be opened, is not
/// RGB or is not a matrix/TRC profile, or when the camera matrix is
/// singular.
pub fn prepare_input(decoded: Decoded) -> Result<PreparedInput, CmsError> {
    let Decoded {
        image,
        transfer,
        xyz_to_camera,
        matrix_white,
        mut warnings,
        ..
    } = decoded;
    let (linear, camera_to_working) = match transfer {
        Transfer::Icc(icc) => {
            let (linear, matrix) = negg_cms::icc_linearise_native(&icc, &image)?;
            (linear, Some(matrix))
        }
        Transfer::Gamma22Assumed => {
            let linear = negg_cms::gamma22_to_linear(&image);
            warnings.push(
                "untagged TIFF has no camera matrix; the output stays in the scan's own \
                 colour space"
                    .into(),
            );
            (linear, None)
        }
        Transfer::Linear => match xyz_to_camera {
            Some(m) => {
                let white = matrix_white.unwrap_or_else(|| {
                    warnings.push(
                        "camera matrix has no recorded illuminant white; assuming D65".into(),
                    );
                    D65_XYZ
                });
                (image, Some(negg_cms::camera_to_rec2020(m, white)?))
            }
            None => {
                warnings.push(
                    "no camera matrix for this input; the output stays in camera RGB, not \
                     Rec.2020"
                        .into(),
                );
                (image, None)
            }
        },
    };
    Ok(PreparedInput {
        linear,
        camera_to_working,
        notes: warnings,
    })
}

/// Loads `--settings` when given (a settings file or a sidecar, see
/// [`parse_settings`]), else the defaults, and applies the
/// `--space` override to `colour.output_space` so that the sidecar records
/// the space that was actually written.
pub(crate) fn load_settings(args: &OutputArgs) -> Result<Settings, ConvertError> {
    let mut settings = match &args.settings {
        Some(path) => {
            let wrap = |source| ConvertError::Settings {
                path: path.clone(),
                source,
            };
            let text = std::fs::read_to_string(path).map_err(|e| wrap(SettingsError::Io(e)))?;
            parse_settings(&text).map_err(wrap)?
        }
        None => Settings::default(),
    };
    if let Some(space) = args.space {
        settings.colour.output_space = space.into();
    }
    Ok(settings)
}

/// The output directory for `input`: `--out` when given, else the input's
/// own directory (the current directory for a bare file name). Created if
/// absent.
pub(crate) fn output_dir(args: &OutputArgs, input: &Path) -> Result<PathBuf, ConvertError> {
    let dir = match &args.out {
        Some(dir) => dir.clone(),
        None => match input.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
            _ => PathBuf::from("."),
        },
    };
    std::fs::create_dir_all(&dir).map_err(|source| ConvertError::Io {
        path: dir.clone(),
        source,
    })?;
    Ok(dir)
}

/// The output image path for `input` in `out_dir`: `<stem>.negg.tif` or
/// `<stem>.negg.jpg`, parallel to the `<stem>.negg.json` sidecar, so an
/// output never shares a name with a source.
pub fn output_path(input: &Path, out_dir: &Path, format: Format) -> PathBuf {
    let mut name = input
        .file_stem()
        .map(|s| s.to_os_string())
        .unwrap_or_default();
    name.push(NEGG_INFIX);
    name.push(format.extension());
    out_dir.join(name)
}

/// Whether `a` and `b` name the same file. When both exist they are
/// compared by device and inode on Unix (which catches symbolic and hard
/// links, `..` and case-insensitive file systems) and by canonical path
/// elsewhere; otherwise the paths are compared as given.
fn same_file(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(ma), Ok(mb)) = (std::fs::metadata(a), std::fs::metadata(b)) {
            return ma.dev() == mb.dev() && ma.ino() == mb.ino();
        }
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Per-stage wall-clock times of one conversion, in milliseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Timing {
    /// Decode, including linearisation and the camera matrix.
    pub decode_ms: f64,
    /// Analysis (steps 3 to 6 on the grid) and the fused render.
    pub render_ms: f64,
    /// The working-to-output transform and quantisation.
    pub transform_ms: f64,
    /// Writing the image and the sidecar.
    pub write_ms: f64,
}

impl Timing {
    /// The sum of the stages.
    pub fn total_ms(&self) -> f64 {
        self.decode_ms + self.render_ms + self.transform_ms + self.write_ms
    }
}

impl fmt::Display for Timing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "decode {:.0} ms, analyse+render {:.0} ms, transform {:.0} ms, write {:.0} ms, \
             total {:.0} ms",
            self.decode_ms,
            self.render_ms,
            self.transform_ms,
            self.write_ms,
            self.total_ms()
        )
    }
}

/// What one successful conversion produced.
#[derive(Debug)]
pub struct Converted {
    /// The output image.
    pub output: PathBuf,
    /// The sidecar beside the source.
    pub sidecar: PathBuf,
    /// The render, kept for `--dump-stats`.
    pub render: FrameRender,
    /// Whether step 7 was applied.
    pub matrix_applied: bool,
    /// Per-stage times.
    pub timing: Timing,
}

/// Checks, before anything is decoded or written, that converting `input`
/// to `output` cannot write over the source and that its sidecar is not
/// the roll sidecar (R-roll).
///
/// # Errors
///
/// [`ConvertError::WouldOverwriteSource`] or
/// [`ConvertError::RollSidecarClash`].
pub fn check_targets(input: &Path, output: &Path) -> Result<(), ConvertError> {
    let sidecar = negg_io::sidecar_path(input);
    if sidecar.file_name() == Some(std::ffi::OsStr::new(negg_io::ROLL_SIDECAR_NAME)) {
        return Err(ConvertError::RollSidecarClash(input.to_path_buf()));
    }
    for target in [output, sidecar.as_path()] {
        if same_file(target, input) {
            return Err(ConvertError::WouldOverwriteSource {
                input: input.to_path_buf(),
                target: target.to_path_buf(),
            });
        }
    }
    Ok(())
}

/// What a frame is converted with besides its settings: the roll context
/// (rig, film correction, roll settings), the roll pool, the overrides to
/// keep in its sidecar and the roll sidecar name to record. The default is
/// a phase 1 conversion.
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameOptions<'a> {
    /// The rig and film corrections and the roll settings; `None` renders
    /// exactly as phase 1 ([`negg_render::render_frame`]).
    pub ctx: Option<&'a RollContext>,
    /// The roll pool, filtered by the roll's `pool_*` flags at render.
    pub pool: Option<&'a RollPool>,
    /// The frame's overrides, kept in the sidecar written (R-lock).
    pub overrides: Option<&'a FrameOverrides>,
    /// Recorded as the sidecar's `roll` (the roll sidecar's file name) when
    /// the frame is rendered in roll mode.
    pub roll: Option<&'a str>,
}

/// Converts one frame: decode, prepare, render, export and write the image
/// (at [`output_path`]) and the sidecar. Decoder warnings and notes go to
/// stderr prefixed with `negg convert: <name>:`. Exactly
/// [`convert_frame_with`] with [`FrameOptions::default`], the phase 1
/// conversion.
///
/// # Errors
///
/// [`ConvertError::WouldOverwriteSource`] or
/// [`ConvertError::RollSidecarClash`], before anything is decoded or
/// written, when the output or the sidecar path resolves to the input or
/// the sidecar would be the roll sidecar; otherwise any [`ConvertError`]
/// other than the usage variants and [`ConvertError::DuplicateOutput`].
pub fn convert_frame(
    input: &Path,
    out_dir: &Path,
    settings: &Settings,
    format: Format,
) -> Result<Converted, ConvertError> {
    convert_frame_with(input, out_dir, settings, format, &FrameOptions::default())
}

/// [`convert_frame`] with a roll context, pool and overrides (R-roll,
/// R-lock): the frame is prepared with [`prepare_frame`] (the rig's flat
/// field and unmix applied) and rendered with [`render_in_roll`] when
/// `opts.ctx` is given, else exactly as phase 1. The sidecar records
/// `settings`, the estimates, `opts.overrides` and `opts.roll`.
///
/// # Errors
///
/// As [`convert_frame`].
pub fn convert_frame_with(
    input: &Path,
    out_dir: &Path,
    settings: &Settings,
    format: Format,
    opts: &FrameOptions,
) -> Result<Converted, ConvertError> {
    let name = input
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let output = output_path(input, out_dir, format);
    let sidecar = negg_io::sidecar_path(input);
    check_targets(input, &output)?;
    let mut timing = Timing::default();

    let t = Instant::now();
    let prepared = prepare_frame(input, opts.ctx.and_then(|c| c.rig.as_ref()))?;
    for note in &prepared.notes {
        eprintln!("negg convert: {name}: {note}");
    }
    timing.decode_ms = t.elapsed().as_secs_f64() * 1e3;

    let t = Instant::now();
    let matrix_applied = prepared.camera_to_working.is_some();
    let render = match opts.ctx {
        Some(ctx) => render_in_roll(&prepared, settings, ctx, opts.pool)?,
        None => render_frame(&prepared.linear, settings, prepared.camera_to_working)?,
    };
    drop(prepared);
    timing.render_ms = t.elapsed().as_secs_f64() * 1e3;

    let space: OutputSpace = settings.colour.output_space;
    let t = Instant::now();
    let transform = negg_cms::working_to_output(space, Intent::Perceptual)?;
    let icc = negg_cms::profile_bytes(space)?;
    let (w, h) = (render.image.width(), render.image.height());
    let encoded = match format {
        Format::Tiff16 => Encoded::U16(transform.apply(&render.image)),
        Format::Jpeg => Encoded::U8(transform.apply_u8(&render.image)),
    };
    timing.transform_ms = t.elapsed().as_secs_f64() * 1e3;

    let t = Instant::now();
    match &encoded {
        Encoded::U16(rgb16) => {
            let opts = TiffOptions {
                icc: Some(icc),
                ..TiffOptions::default()
            };
            negg_io::write_tiff16(&output, w, h, rgb16, &opts)?;
        }
        Encoded::U8(rgb8) => {
            let opts = JpegOptions {
                icc: Some(icc),
                ..JpegOptions::default()
            };
            negg_io::write_jpeg8(&output, w, h, rgb8, &opts)?;
        }
    }
    drop(encoded);
    let mut record = Sidecar::new(
        input,
        settings.clone(),
        Some(render.estimates),
        env!("CARGO_PKG_VERSION"),
    );
    if let Some(overrides) = opts.overrides {
        record.overrides = overrides.clone();
    }
    record.roll = opts.roll.map(str::to_owned);
    negg_io::write_sidecar(&sidecar, &record)?;
    timing.write_ms = t.elapsed().as_secs_f64() * 1e3;

    Ok(Converted {
        output,
        sidecar,
        render,
        matrix_applied,
        timing,
    })
}

/// The quantised pixels for one output format.
enum Encoded {
    U16(Vec<u16>),
    U8(Vec<u8>),
}

/// The `--dump-stats` block for one frame: the estimates of steps 3 to 6,
/// the grid and image sizes, and whether the step 7 matrix was applied.
pub fn format_stats(input: &Path, converted: &Converted) -> String {
    let r = &converted.render;
    let e = &r.estimates;
    let three = |v: [f32; 3]| format!("[{:.4}, {:.4}, {:.4}]", v[0], v[1], v[2]);
    let mut s = String::new();
    s.push_str(&format!("frame: {}\n", input.display()));
    s.push_str(&format!(
        "dimensions: {}x{}\n",
        r.image.width(),
        r.image.height()
    ));
    s.push_str(&format!("grid cells: {}\n", r.grid_cells));
    s.push_str(&format!(
        "dmin: {} ({})\n",
        three(e.dmin),
        source_name(&e.dmin_source)
    ));
    s.push_str(&format!("dmax: {}\n", three(e.dmax)));
    s.push_str(&format!(
        "gamma: {} ({})\n",
        three(e.gamma),
        source_name(&e.gamma_source)
    ));
    s.push_str(&format!("anchor: {:.4}\n", e.anchor));
    s.push_str(&format!("x0: {:.4}\n", e.x0));
    s.push_str(&format!(
        "cast: slope_r {} offset_r {} slope_b {} offset_b {} cells_used {}\n",
        four(e.cast.slope_r),
        four(e.cast.offset_r),
        four(e.cast.slope_b),
        four(e.cast.offset_b),
        e.cast.cells_used
    ));
    s.push_str(&format!(
        "matrix: {}\n",
        if converted.matrix_applied {
            "camera to Rec.2020 applied"
        } else {
            "none"
        }
    ));
    s
}

/// `v` to four decimals, with a value that rounds to zero printed as
/// `0.0000` rather than `-0.0000`.
pub(crate) fn four(v: f32) -> String {
    let rounded = (f64::from(v) * 1e4).round() / 1e4 + 0.0;
    format!("{rounded:.4}")
}

/// The snake_case name of a serde-renamed enum value, as the sidecar
/// writes it.
pub(crate) fn source_name<T: serde::Serialize>(source: &T) -> String {
    serde_json::to_value(source)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// Whether `path` names a file `negg batch` converts: its extension is one
/// of [`BATCH_EXTENSIONS`] in any letter case and its name does not
/// contain `.negg.` (a sidecar or a previous output).
pub fn is_batch_input(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    if name.contains(NEGG_INFIX) {
        return false;
    }
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|e| BATCH_EXTENSIONS.contains(&e.as_str()))
}

/// The files `negg batch` would convert in `dir`, sorted by file name.
///
/// # Errors
///
/// [`ConvertError::NotADirectory`] when `dir` is not a directory,
/// [`ConvertError::Io`] when it cannot be read.
pub fn batch_inputs(dir: &Path) -> Result<Vec<PathBuf>, ConvertError> {
    if !dir.is_dir() {
        return Err(ConvertError::NotADirectory(dir.to_path_buf()));
    }
    let io = |source| ConvertError::Io {
        path: dir.to_path_buf(),
        source,
    };
    let mut inputs: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(io)?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<Result<_, _>>()
        .map_err(io)?;
    inputs.retain(|p| p.is_file() && is_batch_input(p));
    inputs.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    Ok(inputs)
}

/// The outcome of `negg batch`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BatchSummary {
    /// Frames found in the directory.
    pub found: usize,
    /// Frames converted.
    pub converted: usize,
    /// Frames that failed; each error was printed as it happened.
    pub failed: usize,
}

impl fmt::Display for BatchSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "converted {} of {} frame{} ({} failed)",
            self.converted,
            self.found,
            if self.found == 1 { "" } else { "s" },
            self.failed
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use negg_types::{CastCorrection, DminSource, Estimates, GammaSource};

    #[test]
    fn batch_inputs_are_filtered_by_extension_in_any_case() {
        for ok in [
            "a.dng", "A.DNG", "b.CR2", "c.cr3", "d.NEF", "e.arw", "f.raf", "g.tif", "h.TIFF",
            "i.rw2", "j.3fr",
        ] {
            assert!(is_batch_input(Path::new(ok)), "{ok}");
        }
        for no in [
            "a.negg.json",
            "a.json",
            "a.jpg",
            "a.png",
            "a.txt",
            "a",
            "a.tif.negg.json",
            "a.negg.tif",
            "a.negg.jpg",
        ] {
            assert!(!is_batch_input(Path::new(no)), "{no}");
        }
    }

    #[test]
    fn outputs_are_named_beside_the_sidecar_and_never_match_the_source() {
        let out = Path::new("out");
        assert_eq!(
            output_path(Path::new("roll/a.tif"), out, Format::Tiff16),
            Path::new("out/a.negg.tif")
        );
        assert_eq!(
            output_path(Path::new("a.dng"), out, Format::Jpeg),
            Path::new("out/a.negg.jpg")
        );
        // With the default output directory a TIFF source keeps its name.
        let input = Path::new("roll/a.tif");
        assert_ne!(output_path(input, Path::new("roll"), Format::Tiff16), input);
    }

    #[cfg(unix)]
    #[test]
    fn convert_frame_refuses_to_write_over_its_input() {
        let dir = std::env::temp_dir().join(format!("negg-overwrite-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // `y.negg.tif` (the output of `y`) is a link to `y` itself, once as
        // a symbolic link and once as a hard link.
        let y = dir.join("y");
        std::fs::write(&y, b"source").unwrap();
        let output = dir.join("y.negg.tif");
        for hard in [false, true] {
            let _ = std::fs::remove_file(&output);
            if hard {
                std::fs::hard_link(&y, &output).unwrap();
            } else {
                std::os::unix::fs::symlink(&y, &output).unwrap();
            }
            let err = convert_frame(&y, &dir, &Settings::default(), Format::Tiff16).unwrap_err();
            assert!(
                matches!(err, ConvertError::WouldOverwriteSource { .. }),
                "{err:?}"
            );
            assert!(!err.is_usage_error());
            assert!(err.to_string().starts_with("refusing to write"), "{err}");
            assert_eq!(std::fs::read(&y).unwrap(), b"source");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn four_decimals_never_print_a_negative_zero() {
        assert_eq!(four(-1e-9), "0.0000");
        assert_eq!(four(-0.0), "0.0000");
        assert_eq!(four(-0.0002), "-0.0002");
        assert_eq!(four(1.05), "1.0500");
    }

    #[test]
    fn source_names_are_snake_case() {
        assert_eq!(source_name(&DminSource::Border), "border");
        assert_eq!(source_name(&DminSource::Percentile), "percentile");
        assert_eq!(source_name(&GammaSource::Range), "range");
        assert_eq!(source_name(&GammaSource::Fixed), "fixed");
    }

    #[test]
    fn stats_block_names_every_estimate() {
        let converted = Converted {
            output: PathBuf::from("out/f.negg.tif"),
            sidecar: PathBuf::from("f.negg.json"),
            render: FrameRender {
                image: Image::new(4, 3),
                estimates: Estimates {
                    dmin: [0.2, 0.5, 0.7],
                    dmin_source: DminSource::Border,
                    dmax: [1.9, 2.2, 2.4],
                    gamma: [1.7, 1.7, 1.7],
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
                    cast_source: negg_types::CastSource::Fit,
                },
                grid_cells: 12,
            },
            matrix_applied: true,
            timing: Timing::default(),
        };
        let s = format_stats(Path::new("f.dng"), &converted);
        assert!(s.contains("frame: f.dng\n"), "{s}");
        assert!(s.contains("dimensions: 4x3\n"), "{s}");
        assert!(s.contains("grid cells: 12\n"), "{s}");
        assert!(
            s.contains("dmin: [0.2000, 0.5000, 0.7000] (border)\n"),
            "{s}"
        );
        assert!(s.contains("dmax: [1.9000, 2.2000, 2.4000]\n"), "{s}");
        assert!(
            s.contains("gamma: [1.7000, 1.7000, 1.7000] (range)\n"),
            "{s}"
        );
        assert!(s.contains("anchor: -0.4200\n"), "{s}");
        assert!(s.contains("x0: 1.2500\n"), "{s}");
        assert!(
            s.contains(
                "cast: slope_r 1.0500 offset_r -0.0100 slope_b 0.9700 offset_b 0.0200 cells_used 123\n"
            ),
            "{s}"
        );
        assert!(s.contains("matrix: camera to Rec.2020 applied\n"), "{s}");
        assert!(!s.contains('\u{2014}') && !s.contains('\u{2013}'));
    }

    #[test]
    fn timing_and_summary_display() {
        let t = Timing {
            decode_ms: 10.4,
            render_ms: 20.0,
            transform_ms: 5.0,
            write_ms: 4.6,
        };
        assert!((t.total_ms() - 40.0).abs() < 1e-9);
        assert_eq!(
            t.to_string(),
            "decode 10 ms, analyse+render 20 ms, transform 5 ms, write 5 ms, total 40 ms"
        );
        let s = BatchSummary {
            found: 3,
            converted: 2,
            failed: 1,
        };
        assert_eq!(s.to_string(), "converted 2 of 3 frames (1 failed)");
        let one = BatchSummary {
            found: 1,
            converted: 1,
            failed: 0,
        };
        assert_eq!(one.to_string(), "converted 1 of 1 frame (0 failed)");
    }

    #[test]
    fn settings_text_is_a_settings_file_or_a_sidecar() {
        let s = parse_settings(r#"{"print": {"grade": 90}}"#).unwrap();
        assert_eq!(s.print.grade, 90.0);
        let mut sidecar_settings = Settings::default();
        sidecar_settings.print.grade = 60.0;
        let sidecar = Sidecar::new(Path::new("a.dng"), sidecar_settings.clone(), None, "0.0.0");
        let text = serde_json::to_string(&sidecar).unwrap();
        assert_eq!(parse_settings(&text).unwrap(), sidecar_settings);
        // A sidecar that does not migrate is reported as one.
        let err = parse_settings(r#"{"settings": {}, "schema_version": 99}"#).unwrap_err();
        assert!(matches!(err, SettingsError::Sidecar(_)), "{err:?}");
        assert!(err.to_string().starts_with("read as a sidecar:"), "{err}");
        // The empty object is a settings file with every field defaulted.
        assert_eq!(parse_settings("{}").unwrap(), Settings::default());
        // Neither: rejected.
        let err = parse_settings(r#"{"grade": 60}"#).unwrap_err();
        assert!(matches!(err, SettingsError::Unrecognised), "{err:?}");
        assert!(err.to_string().contains("colour"), "{err}");
        assert!(matches!(
            parse_settings("[1, 2]"),
            Err(SettingsError::Json(_))
        ));
        assert!(matches!(
            parse_settings(r#"{"print": {"grade": 500}}"#),
            Err(SettingsError::Invalid(_))
        ));
    }

    #[test]
    fn a_v1_settings_file_is_migrated_to_the_current_schema() {
        let v1 = parse_settings(r#"{"schema_version": 1, "print": {"grade": 70}}"#).unwrap();
        let v2 = parse_settings(r#"{"schema_version": 2, "print": {"grade": 70}}"#).unwrap();
        assert_eq!(v1.schema_version, SETTINGS_SCHEMA_VERSION);
        assert_eq!(v1, v2);
        assert_eq!(v1.content_hash(), v2.content_hash());
    }

    #[test]
    fn usage_errors_are_classified() {
        let e = ConvertError::NotADirectory(PathBuf::from("x"));
        assert!(e.is_usage_error());
        assert_eq!(e.to_string(), "x is not a directory");
        let e = ConvertError::Settings {
            path: PathBuf::from("s.json"),
            source: SettingsError::Invalid(negg_types::Error::Invalid("grade".into())),
        };
        assert!(e.is_usage_error());
        assert_eq!(e.to_string(), "settings s.json: invalid settings: grade");
        let e = ConvertError::Render(negg_types::Error::Invalid("x0".into()));
        assert!(!e.is_usage_error());
        assert!(e.to_string().starts_with("render:"));
    }
}
