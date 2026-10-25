// SPDX-License-Identifier: Apache-2.0

//! One frame's conversion: decode, prepare, render, export and write the
//! sidecar, plus the naming and safety checks `negg convert` and
//! `negg batch` share.
//!
//! This module orchestrates and does I/O only. The maths is in the engine
//! crates: linearisation and the step 7 matrix in `negg-cms`, the
//! analysis and render in `negg-render` (one pipeline, ground rule 3),
//! the export transform in `negg-cms` and the writers in `negg-io`. The
//! pixels stay `f32` until [`negg_cms::OutputTransform`] quantises them
//! once for the writer (ground rule 4).
//!
//! The input is brought to linear light by its [`Transfer`]:
//!
//! - RAW (`Linear`): linear camera RGB through steps 3 to 6. Step 7
//!   reads the print channels as Rec.709 primaries with a D65 white and
//!   enters Rec.2020 with [`negg_cms::REC709_TO_REC2020`] (BT.2087;
//!   ADR 0026, Ruling 96). The camera's scene matrix is not applied: the
//!   print channels are tone curves of dye densities, not camera
//!   responses. The decoder still records it in `Decoded` for the rig and
//!   print characterisation work.
//! - TIFF with an ICC profile (`Icc`): linearised per channel through
//!   that profile's own TRC with [`negg_cms::icc_linearise_native`], so
//!   the scan stays in its own primaries through steps 3 to 6; the step 7
//!   matrix out of those primaries comes from the profile's colorants
//!   (ADR 0012). Only matrix/TRC profiles are accepted.
//! - Integer TIFF with no profile (`Gamma22Assumed`): undone with
//!   [`negg_cms::gamma22_to_linear`] and the decoder's warning is kept as
//!   a note. An untagged TIFF carries no camera matrix, so step 7 is
//!   skipped and the output is in whatever space the scan was in.
//!
//! Nothing here prints: decoder warnings and notes are returned in
//! [`Converted::notes`] (and reported as they happen by the flows in
//! [`crate::flow`]).

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use negg_cms::{CmsError, Intent};
use negg_decode::{Decoded, Transfer};
use negg_io::{JpegOptions, Sidecar, TiffOptions};
use negg_render::{FrameRender, render_frame};
use negg_types::settings::OutputSpace;
use negg_types::{FrameOverrides, Image, RollPool, Settings};

use crate::error::ConvertError;
use crate::roll::{RollContext, prepare_frame, render_in_roll};

/// Output file formats.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Format {
    /// 16-bit TIFF with Deflate compression and the ICC profile embedded.
    #[default]
    Tiff16,
    /// 8-bit JPEG at quality 92 with the ICC profile embedded.
    Jpeg,
}

impl Format {
    /// The file extension written for this format.
    pub fn extension(self) -> &'static str {
        match self {
            Format::Tiff16 => "tif",
            Format::Jpeg => "jpg",
        }
    }
}

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

/// A decoded input brought to linear light, ready for
/// [`negg_render::render_frame`].
#[derive(Debug)]
pub struct PreparedInput {
    /// Linear camera RGB (RAW, untagged TIFF) or linear RGB in the
    /// primaries of the embedded ICC profile (TIFF with a profile).
    pub linear: Image,
    /// The step 7 matrix into Rec.2020 when the input has one: the
    /// embedded profile's colorants for a tagged TIFF, the BT.2087
    /// Rec.709-to-Rec.2020 matrix for RAW (ADR 0026), `None` for an
    /// untagged TIFF.
    pub camera_to_working: Option<[[f32; 3]; 3]>,
    /// What `camera_to_working` is, as `--dump-stats` prints it; `None`
    /// exactly when `camera_to_working` is.
    pub matrix_description: Option<&'static str>,
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
/// RGB or is not a matrix/TRC profile.
pub fn prepare_input(decoded: Decoded) -> Result<PreparedInput, CmsError> {
    let Decoded {
        image,
        transfer,
        mut warnings,
        ..
    } = decoded;
    let (linear, camera_to_working, matrix_description) = match transfer {
        Transfer::Icc(icc) => {
            let (linear, matrix) = negg_cms::icc_linearise_native(&icc, &image)?;
            (linear, Some(matrix), Some("camera to Rec.2020 applied"))
        }
        Transfer::Gamma22Assumed => {
            let linear = negg_cms::gamma22_to_linear(&image);
            warnings.push(
                "untagged TIFF has no camera matrix; the output stays in the scan's own \
                 colour space"
                    .into(),
            );
            (linear, None, None)
        }
        // RAW (Ruling 96, ADR 0026): the print channels are synthetic tone
        // curves of dye densities, not camera responses to scene light, so
        // the camera's scene matrix does not describe them. They are read
        // as Rec.709 primaries with a D65 white. The decoder's camera
        // matrix (`Decoded::xyz_to_camera`) is left in `Decoded` for the
        // rig and print characterisation work; nothing here uses it.
        Transfer::Linear => (
            image,
            Some(negg_cms::REC709_TO_REC2020),
            Some("Rec.709 print primaries to Rec.2020 applied (BT.2087)"),
        ),
    };
    Ok(PreparedInput {
        linear,
        camera_to_working,
        matrix_description,
        notes: warnings,
    })
}

/// The output directory for `input`: `out_dir` when given, else the
/// input's own directory (the current directory for a bare file name).
/// Created if absent.
pub(crate) fn output_dir(out_dir: Option<&Path>, input: &Path) -> Result<PathBuf, ConvertError> {
    let dir = match out_dir {
        Some(dir) => dir.to_path_buf(),
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
    /// Decode, including linearisation and the step 7 matrix.
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
    /// What step 7 applied, as `--dump-stats` prints it; `None` when no
    /// matrix was applied.
    pub matrix_applied: Option<&'static str>,
    /// Per-stage times.
    pub timing: Timing,
    /// The decoder's warnings and any note about the matrix, in the order
    /// they arose (the CLI prints each as `negg convert: <name>: <note>`).
    pub notes: Vec<String>,
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
/// (at [`output_path`]) and the sidecar. Decoder warnings and notes are
/// returned in [`Converted::notes`]. Exactly [`convert_frame_with`] with
/// [`FrameOptions::default`], the phase 1 conversion.
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
/// `opts.ctx` is given, else exactly as phase 1. `settings.geometry` is
/// then applied with [`negg_types::Geometry::apply`] (crop, rotate, flip;
/// skipped when it is the identity) before the output transform, so the
/// returned render's image is the exported framing. The sidecar records
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
    convert_frame_noting(input, out_dir, settings, format, opts, &mut |_| {})
}

/// [`convert_frame_with`], also handing each note to `on_note` as soon as
/// the frame is prepared, so that a caller can report it even when the
/// conversion then fails (the flows in [`crate::flow`] do).
pub(crate) fn convert_frame_noting(
    input: &Path,
    out_dir: &Path,
    settings: &Settings,
    format: Format,
    opts: &FrameOptions,
    on_note: &mut dyn FnMut(&str),
) -> Result<Converted, ConvertError> {
    let output = output_path(input, out_dir, format);
    let sidecar = negg_io::sidecar_path(input);
    check_targets(input, &output)?;
    let mut timing = Timing::default();

    let t = Instant::now();
    let mut prepared = prepare_frame(input, opts.ctx.and_then(|c| c.rig.as_ref()))?;
    for note in &prepared.notes {
        on_note(note);
    }
    let notes = std::mem::take(&mut prepared.notes);
    timing.decode_ms = t.elapsed().as_secs_f64() * 1e3;

    let t = Instant::now();
    let matrix_applied = prepared.matrix_description;
    let mut render = match opts.ctx {
        Some(ctx) => render_in_roll(&prepared, settings, ctx, opts.pool)?,
        None => render_frame(&prepared.linear, settings, prepared.camera_to_working)?,
    };
    drop(prepared);
    // Step 8 geometry (Ruling 70): crop, rotate, flip after the render and
    // before the output transform. The identity skips the copy, so a frame
    // with no geometry exports exactly as before.
    if !settings.geometry.is_identity() {
        render.image = settings.geometry.apply(&render.image)?;
    }
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
        notes,
    })
}

/// The quantised pixels for one output format.
enum Encoded {
    U16(Vec<u16>),
    U8(Vec<u8>),
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
    fn the_default_format_is_a_16_bit_tiff() {
        assert_eq!(Format::default(), Format::Tiff16);
        assert_eq!(Format::Tiff16.extension(), "tif");
        assert_eq!(Format::Jpeg.extension(), "jpg");
    }
}
