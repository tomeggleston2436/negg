// SPDX-License-Identifier: Apache-2.0

//! The [`Session`]: one open roll as the desktop app and the C ABI drive it
//! (Rulings 73 to 83, 87, 89, 122, 124, 125).
//!
//! A session opens a roll directory without decoding or writing anything,
//! then runs pass 1 one frame per [`Session::analyse_next`] call: each
//! frame is decoded, prepared with the rig, reduced to its whole-frame
//! analysis grid (kept, Ruling 87) and analysed under its geometry; the
//! analysed frames are then pooled, exactly as `negg batch` pools them.
//! Every preview (tiles, thumbnails, scopes) renders with the frame's
//! [`FramePlan`], made by [`plan_frame`] from the cached grid with the
//! pool, so the preview and the export share one plan (Ruling 73).
//!
//! Edits are whole values ([`FrameOverrides`] for a frame, [`RollSettings`]
//! for the roll; Ruling 79). A preview changes what is drawn and nothing
//! else; a commit is recorded in one linear history, marks the state dirty
//! and, by the stale-pool rule (Ruling 81), marks the pool pending when the
//! edit changes what the pool sees. A commit returns at once with the
//! frame planned on the current pool; the pending re-pool runs in the next
//! [`Session::analyse_next`] call (which the host drives, as it drives pass
//! 1), and before any flush, lock or export (Ruling 124).
//! [`Session::flush`] writes the dirty sidecars with the data safety rules
//! of Rulings 80 and 122, and once the pool is final every other sidecar a
//! roll edit made out of date (Ruling 139); a session that only browsed
//! writes nothing (Ruling 141). [`Session::export`] checks every sidecar on
//! disk, flushes and then calls the CLI's own flows (Ruling 82), so an
//! export is byte-identical to `negg batch` or `negg convert` on the same
//! directory.
//!
//! Memory per frame (Rulings 87 and 125): the analysis grid (block medians
//! and their coefficient of variation) and the pool input's density, which
//! is the whole grid density when the frame has no analysis area and no
//! crop. The whole density of such a frame is rebuilt from the grid when a
//! plan needs it.
//!
//! Before pass 1 (Ruling 134) the app can show each frame's decoded
//! negative, not inverted ([`Session::negative_view`],
//! [`Session::negative_tile`], [`Session::negative_thumbnail_jpeg`]), and
//! set the roll's analysis area ([`Session::set_roll_analysis_area`]) so
//! that pass 1 analyses only the film. A frame decoded for its negative
//! keeps its analysis grid as its pass 1 data, so pass 1 does not decode it
//! again (Ruling 140).
//!
//! Orchestration only: every pixel value is computed by `negg-render`
//! (render, pyramid, scopes) and `negg-cms` (the sRGB display transform).
//! Turning RGB into RGBA with alpha 255 and applying the geometry
//! (`Geometry::apply`) only move pixels (Ruling 70).

use std::borrow::Cow;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

#[cfg(feature = "gpu")]
use negg_render::gpu::Gpu;

use negg_cms::{Intent, OutputTransform};
use negg_film::{AnalysisGrid, analysis_grid};
use negg_io::{JpegOptions, ROLL_SIDECAR_NAME, ROLL_SIDECAR_SCHEMA_VERSION, RollSidecar, Sidecar};
use negg_render::{
    ANALYSIS_GRID_LONG_SIDE, FrameAnalysis, FramePlan, Region, RenderContext, RollFrame,
    analyse_grid, density_grid, negative_display, negative_display_scale, plan_frame, pool_roll,
    pyramid, render_full, render_region,
};
use negg_types::roll::resolve;
use negg_types::settings::{ChannelEstimate, OutputSpace};
use negg_types::{
    ContentHash, CropRect, DensityModel, FilmCorrection, FrameOverrides, Geometry, Image, Preset,
    PrintLook, RigCorrection, RollPool, RollSettings, Settings, dmax_pinned,
};

use crate::convert::{Format, batch_inputs, check_targets, output_path};
use crate::error::{ConvertError, SessionError};
use crate::flow::{FlowEvent, OutputRequest, batch_dir, convert_one};
use crate::presets::preset_frame_overrides;
use crate::roll::{
    LockWhich, build_context, file_name, lock_overrides, overrides_from_bytes, prepare_frame,
    read_overrides, read_roll, roll_from_bytes,
};
use crate::view::{
    ExportFormat, ExportReport, ExportRequest, ExportScope, FrameInfo, FrameStatus, FrameView,
    HistoryView, NegativeView, RollPresetScope, RollView,
};
use crate::wire::{TILE_SIZE, encode_scopes, encode_tile};

/// Most edits the history keeps; the oldest is dropped beyond this
/// (Ruling 79).
pub const HISTORY_LIMIT: usize = 1000;
/// The warning when an analysis area or crop change takes the roll's film
/// base (Dmin) pool off the clear film edge (Ruling 138): the frames'
/// Dmin then comes from their darkest image areas instead.
pub const BORDER_LOST_WARNING: &str = "the analysis area (or crop) leaves out the clear film edge \
     (rebate), so the film base is no longer measured on it but estimated from the image; make \
     the analysis area cover the whole film including its clear edge and leave out only the \
     holder";
/// JPEG quality of thumbnails (Ruling 76).
pub const THUMB_QUALITY: u8 = 85;
/// Scopes measure the first pyramid level whose long side is at most this
/// (Ruling 75).
pub const SCOPES_LONG_SIDE: usize = 1024;

/// A file's modification time and length, `None` when it does not exist.
type Stamp = Option<(SystemTime, u64)>;

/// The stamp of `path` now.
fn stamp(path: &Path) -> Stamp {
    stamp_of(&std::fs::metadata(path).ok()?)
}

/// The stamp of a file's metadata.
fn stamp_of(meta: &std::fs::Metadata) -> Stamp {
    Some((meta.modified().ok()?, meta.len()))
}

/// What the session last saw of a sidecar it read or wrote: its stamp and
/// its bytes (`None` when it did not exist). A sidecar that changed behind
/// the session is never overwritten (Ruling 80). By Ruling 122 a file has
/// changed only when its stamp differs and its bytes differ too, so a
/// program that wrote the very same sidecar (as `negg batch` does after an
/// export) has not changed it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Seen {
    stamp: Stamp,
    bytes: Option<Vec<u8>>,
}

impl Seen {
    /// `path` as it is now: its stamp and its bytes taken from one open
    /// file, so they describe the same file (a writer that replaces it
    /// meanwhile renames a new file into place and leaves the open one as
    /// it was). The bytes are what the caller parses, so the session never
    /// reads a sidecar twice. A file that exists but cannot be read has a
    /// stamp and no bytes.
    fn read(path: &Path) -> Seen {
        let Ok(mut file) = std::fs::File::open(path) else {
            return Seen {
                stamp: stamp(path),
                bytes: None,
            };
        };
        let stamp = file.metadata().ok().as_ref().and_then(stamp_of);
        let mut bytes = Vec::new();
        Seen {
            stamp,
            bytes: file.read_to_end(&mut bytes).ok().map(|_| bytes),
        }
    }

    /// What the session itself wrote: `bytes`, with the stamp of the file
    /// as written (taken before the rename, see
    /// `negg_io::write_file_atomic`), never read back from the path.
    fn written(meta: &std::fs::Metadata, bytes: Vec<u8>) -> Seen {
        Seen {
            stamp: stamp_of(meta),
            bytes: Some(bytes),
        }
    }

    /// True when `path` is as the session last saw it.
    fn unchanged(&self, path: &Path) -> bool {
        stamp(path) == self.stamp || std::fs::read(path).ok() == self.bytes
    }
}

/// What pass 1 keeps of a frame (Ruling 87): its whole-frame analysis
/// grid, its step 7 matrix, its size and the decoder's notes.
#[derive(Debug)]
struct Pass1 {
    grid: AnalysisGrid,
    camera_to_working: Option<[[f32; 3]; 3]>,
    width: usize,
    height: usize,
    notes: Vec<String>,
}

/// The pooled values a plan was made with (`RollPool::pooled` of the roll
/// settings), as bits: Dmin, gamma and cast, each with a presence flag.
type PoolKey = [u32; 14];

/// A plan or thumbnail is cached under the pooled values and the settings
/// hash it was made with (Ruling 124), so a re-pool that changes nothing
/// keeps every cache.
type PlanKey = (PoolKey, ContentHash);

/// A [`PrintLook`] as bits, for the preset thumbnail cache.
type LookKey = [u32; 5];

/// Most preset thumbnails kept per frame.
const PRESET_THUMBS: usize = 16;

/// The cache key bits of `look`.
fn look_key(look: &PrintLook) -> LookKey {
    [
        look.grade.to_bits(),
        look.toe.to_bits(),
        look.shoulder.to_bits(),
        u32::from(look.black_point_compensation),
        look.saturation.to_bits(),
    ]
}

/// One frame of the open roll.
#[derive(Debug)]
struct Frame {
    path: PathBuf,
    source: String,
    /// The committed overrides.
    overrides: FrameOverrides,
    /// Overrides being previewed (a drag), drawn but not committed.
    preview: Option<FrameOverrides>,
    /// Why the frame can never be converted (a duplicate output or the
    /// roll sidecar's name): `negg batch` fails it before pass 1.
    blocked: Option<String>,
    /// Why its existing sidecar could not be read: `negg batch` fails it
    /// before pass 1, and it is never written.
    unreadable: Option<String>,
    /// Why it turned read-only later: its sidecar changed on disk behind
    /// the session. Its overrides were then read again from disk.
    stale: Option<String>,
    /// The sidecar as the session last read or wrote it.
    seen: Seen,
    /// A commit changed it since the last flush.
    dirty: bool,
    /// Its sidecar was written with no estimates because pass 1 had not
    /// reached it; the next flush after pass 1 writes them.
    estimates_missing: bool,
    /// `None` until pass 1 has reached it.
    pass1: Option<Result<Pass1, String>>,
    /// What decoding the frame for its negative before pass 1 made
    /// (Ruling 140): the very data pass 1 makes, with the rig in use then.
    /// Pass 1 takes it instead of decoding the frame again; a change of
    /// profiles drops it.
    pre: Option<Pass1>,
    /// Its pool input: the analysis under its committed geometry. `None`
    /// until pass 1 has reached it.
    analysis: Option<Result<FrameAnalysis, String>>,
    /// The analysis was made under an analysis area or a crop, so its
    /// density is part of the grid; without either it is the whole grid
    /// density.
    analysis_cropped: bool,
    /// A roll edit changed the frame's analysis area; the analysis is
    /// rebuilt before the next pool.
    analysis_stale: bool,
    /// The last plans made, at most two (committed and previewed).
    plans: Vec<(PlanKey, FramePlan)>,
    /// The last thumbnail made.
    thumb: Option<(PlanKey, Vec<u8>)>,
    /// Preset thumbnails ([`Session::preset_thumbnail_jpeg`]), each under
    /// the key of the thumbnail it was made from and its look; only those
    /// of the latest thumbnail key are kept.
    preset_thumbs: Vec<(PlanKey, LookKey, Vec<u8>)>,
    /// The negative display's scale (Ruling 134), once worked out.
    negative_scale: Option<f32>,
    /// The negative thumbnail, once made.
    negative_thumb: Option<Vec<u8>>,
}

impl Frame {
    /// True when the frame's sidecar must never be written.
    fn read_only(&self) -> bool {
        self.blocked.is_some() || self.unreadable.is_some() || self.stale.is_some()
    }

    /// True when `negg batch` would take the frame into pass 1.
    fn in_roll(&self) -> bool {
        self.blocked.is_none() && self.unreadable.is_none()
    }

    /// The overrides drawn: the preview when there is one.
    fn shown(&self) -> &FrameOverrides {
        self.preview.as_ref().unwrap_or(&self.overrides)
    }

    /// True when a flush has something to write for the frame; `pooled`
    /// says whether the roll's pool is final (pass 1 finished and no
    /// re-pool pending), so that a sidecar written without estimates can
    /// now get them.
    fn needs_write(&self, pooled: bool) -> bool {
        !self.read_only()
            && (self.dirty
                || (self.estimates_missing && pooled && matches!(self.pass1, Some(Ok(_)))))
    }
}

/// One recorded edit (Ruling 79): the value before and after, and the
/// content hash of the settings after.
#[derive(Clone, Debug)]
enum Edit {
    Frame {
        frame: usize,
        before: Arc<FrameOverrides>,
        after: Arc<FrameOverrides>,
        hash: ContentHash,
    },
    Roll {
        before: Arc<RollSettings>,
        after: Arc<RollSettings>,
        hash: ContentHash,
    },
    /// A roll preset (spec "Roll preset vs frames with their own look"):
    /// the roll settings and the overrides of every frame whose look it
    /// cleared, as one step. `hash` is the roll settings' after.
    Preset {
        roll_before: Arc<RollSettings>,
        roll_after: Arc<RollSettings>,
        frames: Vec<(usize, Arc<FrameOverrides>, Arc<FrameOverrides>)>,
        hash: ContentHash,
    },
}

/// Where pass 1 is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Analysing frames; the next is `Session::cursor`.
    Frames,
    /// Every frame analysed; the pool is next.
    Pool,
    /// Pass 1 and the first pool finished.
    Done,
}

/// The profiles of `shared.profiles`, loaded.
#[derive(Debug)]
struct Profiles {
    rig: Option<RigCorrection>,
    film: Option<FilmCorrection>,
}

/// The open frame: level 0 (its full linear image after decode, prepare
/// and rig) and the pyramid levels 1 to L (Ruling 73).
#[derive(Debug)]
struct OpenFrame {
    index: usize,
    levels: Vec<Image>,
    /// The frame's step 7 matrix, as its decode gave it.
    camera_to_working: Option<[[f32; 3]; 3]>,
}

/// One step of pass 1 or of a pending re-pool, as
/// [`Session::analyse_next`] reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnalyseStep {
    /// A frame was analysed; `ok` is false when it failed or cannot be
    /// converted.
    Analysed {
        /// The frame index.
        frame: u32,
        /// Whether it was analysed.
        ok: bool,
    },
    /// The analysed frames were pooled (after pass 1, or a pending re-pool
    /// after an edit).
    Pooled,
    /// Nothing is left to do: pass 1 has finished and no re-pool is
    /// pending.
    Done,
}

/// One open roll: cached analysis, pool, history, preview tiles,
/// thumbnails, scopes, sidecar flushing and export. See the module
/// documentation.
#[derive(Debug)]
pub struct Session {
    dir: PathBuf,
    frames: Vec<Frame>,
    /// The committed roll settings.
    roll: RollSettings,
    /// The roll sidecar as last read or written, parsed and as a file.
    roll_disk: Option<RollSidecar>,
    roll_seen: Seen,
    /// A roll commit, or a frame commit that changed the pool's input,
    /// happened since the last flush.
    roll_dirty: bool,
    phase: Phase,
    cursor: usize,
    profiles: Option<Result<Profiles, String>>,
    warnings: Vec<String>,
    pool: Option<RollPool>,
    pool_error: Option<String>,
    /// An edit changed what the pool sees; the pool is made again in the
    /// next `analyse_next`, or before a flush, lock or export.
    pool_pending: bool,
    /// An analysis area or crop changed since the last pool (Ruling 138).
    area_moved: bool,
    /// The last area change took the roll's Dmin pool off the clear film
    /// edge (Ruling 138); see [`BORDER_LOST_WARNING`].
    pool_warning: Option<String>,
    generation: u32,
    history: Vec<Edit>,
    /// Number of edits of `history` applied; the rest is the redo tail.
    applied: usize,
    open: Option<OpenFrame>,
    srgb: Option<OutputTransform>,
    /// The GPU preview (ADR 0036): the device and the display matrix, until
    /// a GPU render fails.
    #[cfg(feature = "gpu")]
    gpu: GpuPreview,
}

/// Whether [`Session::render_tile`] renders on the GPU (ADR 0036).
#[cfg(feature = "gpu")]
#[derive(Debug, Default)]
enum GpuPreview {
    /// Not asked for: tiles render on the CPU.
    #[default]
    Off,
    /// The device and the working-to-sRGB matrix of the display encoding.
    On(Arc<Gpu>, [[f32; 3]; 3]),
    /// A GPU render failed; tiles render on the CPU from then on.
    Failed,
}

/// The session is moved between threads by its hosts.
const _: () = {
    const fn is_send<T: Send>() {}
    is_send::<Session>();
};

impl Session {
    /// Opens the roll in `dir`: its batch inputs in name order, its roll
    /// sidecar (the roll settings; the stored pool is not used, pass 1
    /// pools again) and each frame's sidecar overrides. Nothing is decoded
    /// and nothing is written. A frame whose sidecar cannot be read is
    /// read-only (it is never written); a frame `negg batch` would refuse
    /// before pass 1 (a duplicate output, the roll sidecar's name) is
    /// failed.
    ///
    /// # Errors
    ///
    /// [`SessionError::Convert`] with [`ConvertError::NotADirectory`] or
    /// [`ConvertError::Io`] when the directory cannot be listed, and
    /// [`ConvertError::RollSidecar`] when `roll.negg.json` exists but
    /// cannot be read.
    pub fn open_roll(dir: &Path) -> Result<Session, SessionError> {
        let inputs = batch_inputs(dir)?;
        let roll_path = negg_io::roll_sidecar_path(dir);
        let roll_seen = Seen::read(&roll_path);
        let roll_disk = match (&roll_seen.bytes, roll_seen.stamp) {
            // It exists but could not be read: as `negg batch` reads it.
            (None, Some(_)) => read_roll(&roll_path)?,
            (bytes, _) => roll_from_bytes(&roll_path, bytes.as_deref())?,
        };
        let roll = roll_disk
            .as_ref()
            .map(|r| r.settings.clone())
            .unwrap_or_default();
        // The checks `negg batch` makes before pass 1, in its order.
        let mut produced: HashMap<PathBuf, PathBuf> = HashMap::new();
        let mut frames = Vec::with_capacity(inputs.len());
        for input in inputs {
            let output = output_path(&input, dir, Format::Tiff16);
            let blocked = if let Some(first) = produced.get(&output) {
                Some(
                    ConvertError::DuplicateOutput {
                        output: output.clone(),
                        first: first.clone(),
                    }
                    .to_string(),
                )
            } else {
                check_targets(&input, &output).err().map(|e| e.to_string())
            };
            if blocked.is_none() {
                produced.insert(output, input.clone());
            }
            let sidecar = negg_io::sidecar_path(&input);
            let seen = Seen::read(&sidecar);
            let (overrides, unreadable) = if blocked.is_some() {
                (FrameOverrides::default(), None)
            } else {
                let read = match (&seen.bytes, seen.stamp) {
                    (None, Some(_)) => read_overrides(&input),
                    (bytes, _) => overrides_from_bytes(&sidecar, bytes.as_deref()),
                };
                match read {
                    Ok(o) => (o, None),
                    Err(e) => (FrameOverrides::default(), Some(e.to_string())),
                }
            };
            frames.push(Frame {
                source: file_name(&input),
                path: input,
                overrides,
                preview: None,
                blocked,
                unreadable,
                stale: None,
                seen,
                dirty: false,
                estimates_missing: false,
                pass1: None,
                pre: None,
                analysis: None,
                analysis_cropped: false,
                analysis_stale: false,
                plans: Vec::new(),
                thumb: None,
                preset_thumbs: Vec::new(),
                negative_scale: None,
                negative_thumb: None,
            });
        }
        Ok(Session {
            dir: dir.to_path_buf(),
            frames,
            roll,
            roll_disk,
            roll_seen,
            roll_dirty: false,
            phase: Phase::Frames,
            cursor: 0,
            profiles: None,
            warnings: Vec::new(),
            pool: None,
            pool_error: None,
            pool_pending: false,
            area_moved: false,
            pool_warning: None,
            generation: 0,
            history: Vec::new(),
            applied: 0,
            open: None,
            srgb: None,
            #[cfg(feature = "gpu")]
            gpu: GpuPreview::Off,
        })
    }

    /// One step of pass 1 or of a pending re-pool (Rulings 81 and 124).
    ///
    /// During pass 1 the next frame is decoded (one at a time), prepared
    /// with the rig and analysed, and its grid is cached; after the last
    /// frame the next call pools the roll ([`AnalyseStep::Pooled`]). The
    /// first call loads the roll's profiles; when they cannot be loaded
    /// every frame fails with that error. A commit that changes the
    /// profiles starts pass 1 again.
    ///
    /// After pass 1, a call makes a pending re-pool (an edit changed what
    /// the pool sees) and returns [`AnalyseStep::Pooled`]; with nothing
    /// pending it returns [`AnalyseStep::Done`]. The host calls it until
    /// Done after opening a roll and after every call whose returned view
    /// reports `pool_pending` (`RollView`, `FrameView` and `HistoryView`
    /// all carry it, so `commit_frame`, `commit_roll`, `lock_frame`,
    /// `undo` and `redo` each tell the host; Ruling 126), then fetches its
    /// views again (the generation has moved when the pool changed).
    ///
    /// # Errors
    ///
    /// [`SessionError::Invalid`] from a pooling step when the pool cannot
    /// be made; the frames keep their own estimates and the next call
    /// returns [`AnalyseStep::Done`].
    pub fn analyse_next(&mut self) -> Result<AnalyseStep, SessionError> {
        match self.phase {
            Phase::Done => {
                if !self.pool_pending {
                    return Ok(AnalyseStep::Done);
                }
                self.settle();
                return match &self.pool_error {
                    Some(e) => Err(SessionError::Invalid(format!("pooling the roll: {e}"))),
                    None => Ok(AnalyseStep::Pooled),
                };
            }
            Phase::Pool => {
                self.phase = Phase::Done;
                self.pool_pending = true;
                return self.analyse_next();
            }
            Phase::Frames => {}
        }
        if self.cursor >= self.frames.len() {
            self.phase = Phase::Pool;
            return self.analyse_next();
        }
        self.ensure_profiles();
        let i = self.cursor;
        self.cursor += 1;
        if self.frames[i].blocked.is_none() {
            // The decode the negative view made with these profiles is
            // pass 1's own data (Ruling 140).
            let pre = self.frames[i].pre.take();
            let (pass1, analysis) = match &self.profiles {
                Some(Ok(p)) => {
                    let settings = resolve(&self.roll, &self.frames[i].overrides);
                    let density = p.rig.as_ref().and_then(|r| r.density.as_ref());
                    match pre.map_or_else(|| decode_pass1(&self.frames[i].path, p), Ok) {
                        Ok(pass1) => {
                            let analysis = analyse_pass1(&pass1, &settings, density);
                            (Ok(pass1), analysis)
                        }
                        Err(e) => (Err(e.clone()), Err(e)),
                    }
                }
                Some(Err(e)) => (Err(e.clone()), Err(e.clone())),
                None => unreachable!("loaded above"),
            };
            let f = &mut self.frames[i];
            f.pass1 = Some(pass1);
            f.analysis_cropped = resolve(&self.roll, &f.overrides)
                .geometry
                .analysis_rect()
                .is_some()
                && analysis.is_ok();
            f.analysis = Some(analysis);
            f.analysis_stale = false;
        }
        if self.cursor >= self.frames.len() {
            self.phase = Phase::Pool;
        }
        let ok = matches!(self.frames[i].analysis, Some(Ok(_)));
        Ok(AnalyseStep::Analysed {
            frame: index_u32(i),
            ok,
        })
    }

    /// Runs [`Session::analyse_next`] until [`AnalyseStep::Done`].
    ///
    /// # Errors
    ///
    /// As [`Session::analyse_next`]; pass 1 is finished even then.
    pub fn analyse_all(&mut self) -> Result<(), SessionError> {
        let mut first = None;
        loop {
            match self.analyse_next() {
                Ok(AnalyseStep::Done) => break,
                Ok(_) => {}
                Err(e) => {
                    first.get_or_insert(e);
                }
            }
        }
        first.map_or(Ok(()), Err)
    }

    /// The number of frames in the roll.
    pub fn frame_count(&self) -> u32 {
        index_u32(self.frames.len())
    }

    /// The warnings to show: those from loading the roll's profiles (a
    /// film profile that gives no correction, one measured with another
    /// rig), then [`BORDER_LOST_WARNING`] when the last analysis area or
    /// crop change took the roll's film base pool off the clear film edge
    /// (Ruling 138; also in [`RollView::pool_warning`]).
    pub fn warnings(&self) -> Vec<String> {
        let mut all = self.warnings.clone();
        all.extend(self.pool_warning.iter().cloned());
        all
    }

    /// The roll as the roll browser shows it.
    pub fn roll_view(&self) -> RollView {
        RollView {
            dir: self.dir.to_string_lossy().into_owned(),
            frames: (0..self.frames.len()).map(|i| self.info(i)).collect(),
            roll: self.roll.clone(),
            analysed: self.phase == Phase::Done,
            generation: self.generation,
            history: self.history_view(),
            pool_pending: self.pool_pending,
            pool_warning: self.pool_warning.clone(),
        }
    }

    /// Opens frame `frame` (decode, prepare with the rig, pyramid;
    /// Ruling 73), closing any other open frame, and returns its view.
    ///
    /// # Errors
    ///
    /// [`SessionError::NoFrame`] for an index outside the roll,
    /// [`SessionError::Busy`] when pass 1 has not reached it,
    /// [`SessionError::Invalid`] when it failed pass 1 and
    /// [`SessionError::Convert`] when it cannot be decoded again.
    pub fn frame_view(&mut self, frame: u32) -> Result<FrameView, SessionError> {
        let i = self.index(frame)?;
        self.open_frame(i)?;
        Ok(self.make_view(i))
    }

    /// One preview tile of the frame (Rulings 73 and 74): level `level` of
    /// its pyramid (0 = level 0), the 512-pixel tile at column `tile_x` and
    /// row `tile_y`, rendered with the frame's plan (the previewed
    /// overrides when a drag is in progress) and the rig density model,
    /// turned to sRGB for display, as the tile wire format. The frame is
    /// opened when it is not.
    ///
    /// # Errors
    ///
    /// As [`Session::frame_view`], plus [`SessionError::Invalid`] for a
    /// level or tile outside the frame and [`SessionError::Convert`] when
    /// the frame cannot be planned.
    pub fn render_tile(
        &mut self,
        frame: u32,
        level: u32,
        tile_x: u32,
        tile_y: u32,
    ) -> Result<Vec<u8>, SessionError> {
        let i = self.index(frame)?;
        self.open_frame(i)?;
        let (_, plan) = self.plan(i, true)?;
        self.ensure_srgb()?;
        let camera = self.camera(i);
        let density = self.density();
        let open = self.open.as_ref().expect("opened above");
        let image = open.levels.get(level as usize).ok_or_else(|| {
            SessionError::Invalid(format!("frame {frame} has no pyramid level {level}"))
        })?;
        let t = TILE_SIZE as usize;
        let (w, h) = (image.width(), image.height());
        let (x, y) = (tile_x as usize * t, tile_y as usize * t);
        if x >= w || y >= h {
            return Err(SessionError::Invalid(format!(
                "frame {frame} level {level} has no tile ({tile_x}, {tile_y})"
            )));
        }
        let region = Region {
            x,
            y,
            width: t.min(w - x),
            height: t.min(h - y),
        };
        #[cfg(feature = "gpu")]
        if let GpuPreview::On(gpu, display) = &self.gpu {
            match gpu.render_region_display(
                image,
                region,
                &plan.estimates,
                &plan.coefficients,
                density,
                camera,
                *display,
            ) {
                Ok(rgba) => {
                    return Ok(encode_tile(
                        frame,
                        self.generation,
                        level,
                        tile_x,
                        tile_y,
                        index_u32(region.width),
                        index_u32(region.height),
                        &rgba,
                    ));
                }
                // The CPU path below reports the same inputs' errors.
                Err(negg_render::gpu::GpuError::Invalid(_)) => {}
                Err(_) => {
                    self.gpu = GpuPreview::Failed;
                    return self.render_tile(frame, level, tile_x, tile_y);
                }
            }
        }
        let rendered = render_region(
            image,
            region,
            &plan.estimates,
            &plan.coefficients,
            density,
            camera,
        )?;
        let rgb = self.srgb.as_ref().expect("built above").apply_u8(&rendered);
        Ok(encode_tile(
            frame,
            self.generation,
            level,
            tile_x,
            tile_y,
            index_u32(region.width),
            index_u32(region.height),
            &rgb_to_rgba(&rgb),
        ))
    }

    /// Renders preview tiles ([`Session::render_tile`]) on `gpu` from now
    /// on (ADR 0036): the positive within 1/4096 per channel of the CPU
    /// kernel and the display within one 8-bit step of the CPU tile.
    /// Exports, thumbnails, scopes and negative tiles stay on the CPU. With
    /// `None`, or once a GPU render has failed (the tile is rendered on
    /// the CPU instead and so is every later one), tiles render on the CPU
    /// and this does nothing.
    ///
    /// # Errors
    ///
    /// [`SessionError::Convert`] when the sRGB display matrix cannot be
    /// built.
    #[cfg(feature = "gpu")]
    pub fn set_gpu(&mut self, gpu: Option<Arc<Gpu>>) -> Result<(), SessionError> {
        if matches!(self.gpu, GpuPreview::Failed) {
            return Ok(());
        }
        self.gpu = match gpu {
            Some(g) => GpuPreview::On(g, negg_cms::working_to_output_matrix(OutputSpace::Srgb)?),
            None => GpuPreview::Off,
        };
        Ok(())
    }

    /// Whether preview tiles render on the GPU ([`Session::set_gpu`]).
    #[cfg(feature = "gpu")]
    #[must_use]
    pub fn uses_gpu(&self) -> bool {
        matches!(self.gpu, GpuPreview::On(..))
    }

    /// The frame's thumbnail (Ruling 76): its cached analysis grid rendered
    /// with its plan, its geometry applied, turned to sRGB and encoded as
    /// a JPEG at [`THUMB_QUALITY`] with the sRGB profile. Kept in memory
    /// until the frame's settings or the pooled values change. Does not
    /// open the frame.
    ///
    /// # Errors
    ///
    /// [`SessionError::NoFrame`], [`SessionError::Busy`] before pass 1
    /// reached it, [`SessionError::Invalid`] when it failed pass 1, and
    /// [`SessionError::Convert`] when it cannot be planned or encoded.
    pub fn thumbnail_jpeg(&mut self, frame: u32) -> Result<Vec<u8>, SessionError> {
        let i = self.index(frame)?;
        let (key, plan) = self.plan(i, true)?;
        if let Some((k, jpeg)) = &self.frames[i].thumb
            && *k == key
        {
            return Ok(jpeg.clone());
        }
        let jpeg = self.encode_thumbnail(i, &plan)?;
        self.frames[i].thumb = Some((key, jpeg.clone()));
        Ok(jpeg)
    }

    /// The frame's thumbnail as [`Session::thumbnail_jpeg`] would make it
    /// if its committed settings (never a drag preview in progress) had
    /// `look` applied (the five look fields of
    /// [`PrintLook::apply_to_settings`]; the preset label does not change
    /// a render): the plan is made again from those settings, so the bytes
    /// equal the thumbnail after [`Session::apply_frame_preset`] with the
    /// same look. Kept per frame by its thumbnail key and the look, and
    /// dropped when the frame's thumbnail cache is. Does not open the
    /// frame, and changes nothing else.
    ///
    /// # Errors
    ///
    /// As [`Session::thumbnail_jpeg`], plus [`SessionError::Invalid`] when
    /// the settings with `look` applied fail validation.
    pub fn preset_thumbnail_jpeg(
        &mut self,
        frame: u32,
        look: &PrintLook,
    ) -> Result<Vec<u8>, SessionError> {
        let i = self.index(frame)?;
        self.pass1(i)?;
        // The committed settings, never a drag preview: the client keys
        // these thumbnails on the committed version.
        let committed = resolve(&self.roll, &self.frames[i].overrides);
        let base = (self.pool_key(), committed.content_hash());
        let lk = look_key(look);
        if let Some((_, _, jpeg)) = self.frames[i]
            .preset_thumbs
            .iter()
            .find(|(k, l, _)| *k == base && *l == lk)
        {
            return Ok(jpeg.clone());
        }
        let mut settings = committed;
        let label = settings.print.preset.take();
        look.apply_to_settings("", &mut settings.print);
        settings.print.preset = label;
        validate(&settings)?;
        let plan = self.make_plan(i, &settings)?;
        let jpeg = self.encode_thumbnail_with(i, &plan, &settings.geometry)?;
        let thumbs = &mut self.frames[i].preset_thumbs;
        thumbs.retain(|(k, _, _)| *k == base);
        if thumbs.len() >= PRESET_THUMBS {
            thumbs.remove(0);
        }
        thumbs.push((base, lk, jpeg.clone()));
        Ok(jpeg)
    }

    /// Encodes frame `i`'s thumbnail with `plan` and its shown geometry.
    fn encode_thumbnail(&mut self, i: usize, plan: &FramePlan) -> Result<Vec<u8>, SessionError> {
        let geometry = self.shown_settings(i).geometry;
        self.encode_thumbnail_with(i, plan, &geometry)
    }

    /// Renders frame `i`'s cached analysis grid with `plan`, applies
    /// `geometry`, turns it to sRGB and encodes it as the thumbnail JPEG
    /// (see [`Session::thumbnail_jpeg`]).
    fn encode_thumbnail_with(
        &mut self,
        i: usize,
        plan: &FramePlan,
        geometry: &Geometry,
    ) -> Result<Vec<u8>, SessionError> {
        self.ensure_srgb()?;
        let density = self.density();
        let pass1 = self.pass1(i)?;
        let image = render_full(
            &pass1.grid.median,
            &plan.estimates,
            &plan.coefficients,
            density,
            pass1.camera_to_working,
        )?;
        let image = apply_geometry(geometry, image)?;
        let rgb = self.srgb.as_ref().expect("built above").apply_u8(&image);
        let opts = JpegOptions {
            icc: Some(negg_cms::profile_bytes(OutputSpace::Srgb)?),
            quality: THUMB_QUALITY,
        };
        Ok(negg_io::encode_jpeg8(
            image.width(),
            image.height(),
            &rgb,
            &opts,
        )?)
    }

    /// The frame's scopes (Ruling 75) as the scopes wire format: measured
    /// on the 8-bit sRGB render of the first pyramid level whose long side
    /// is at most [`SCOPES_LONG_SIDE`], with the geometry applied. The
    /// frame is opened when it is not.
    ///
    /// # Errors
    ///
    /// As [`Session::frame_view`], plus [`SessionError::Convert`] when the
    /// frame cannot be planned.
    pub fn scopes(&mut self, frame: u32) -> Result<Vec<u8>, SessionError> {
        let i = self.index(frame)?;
        self.open_frame(i)?;
        let (_, plan) = self.plan(i, true)?;
        self.ensure_srgb()?;
        let geometry = self.shown_settings(i).geometry;
        let camera = self.camera(i);
        let density = self.density();
        let open = self.open.as_ref().expect("opened above");
        let level = open
            .levels
            .iter()
            .position(|l| l.width().max(l.height()) <= SCOPES_LONG_SIDE)
            .unwrap_or(open.levels.len() - 1);
        let image = render_full(
            &open.levels[level],
            &plan.estimates,
            &plan.coefficients,
            density,
            camera,
        )?;
        let image = apply_geometry(&geometry, image)?;
        let rgb = self.srgb.as_ref().expect("built above").apply_u8(&image);
        let scopes = negg_render::scopes(&rgb, image.width(), image.height())?;
        Ok(encode_scopes(frame, self.generation, &scopes))
    }

    /// Previews `overrides` on the frame (a drag, Ruling 79): tiles,
    /// thumbnails and scopes draw with them until the next commit; no
    /// history, no sidecar, no re-pool (the pool is refreshed after the
    /// commit). Returns the new generation.
    ///
    /// # Errors
    ///
    /// [`SessionError::NoFrame`], [`SessionError::Busy`] before pass 1 has
    /// finished, [`SessionError::ReadOnly`] for a read-only frame and
    /// [`SessionError::Invalid`] when the resolved settings fail
    /// validation.
    pub fn preview_frame(
        &mut self,
        frame: u32,
        overrides: FrameOverrides,
    ) -> Result<u32, SessionError> {
        let i = self.index(frame)?;
        self.check_editable(i)?;
        validate(&resolve(&self.roll, &overrides))?;
        let f = &mut self.frames[i];
        f.preview = (overrides != f.overrides).then_some(overrides);
        Ok(self.bump())
    }

    /// Commits `overrides` as the frame's overrides (Ruling 79): recorded
    /// in the history unless equal to the current value (a commit after an
    /// undo drops the redo tail), the frame marked dirty and any preview
    /// cleared. A new crop or analysis buffer rebuilds the frame's pool
    /// input from its cached grid at once; when the edit changes what the
    /// pool sees, the pool is marked pending (Rulings 81 and 124) and the
    /// returned view has the frame planned on the current pool.
    ///
    /// # Errors
    ///
    /// As [`Session::preview_frame`], plus [`SessionError::Invalid`] when a
    /// new crop leaves too little to analyse. Nothing is changed then, and
    /// a preview in progress is kept.
    pub fn commit_frame(
        &mut self,
        frame: u32,
        overrides: FrameOverrides,
    ) -> Result<FrameView, SessionError> {
        let i = self.index(frame)?;
        self.check_editable(i)?;
        if overrides != self.frames[i].overrides {
            let before = Arc::new(self.frames[i].overrides.clone());
            self.apply_frame(i, overrides.clone(), true)?;
            let hash = resolve(&self.roll, &overrides).content_hash();
            self.record(Edit::Frame {
                frame: i,
                before,
                after: Arc::new(overrides),
                hash,
            });
        }
        self.frames[i].preview = None;
        self.bump();
        Ok(self.make_view(i))
    }

    /// Commits `roll` as the roll settings (Ruling 79). A change of a
    /// frame's analysis area, a pool flag or a pool input marks the pool
    /// pending (Rulings 81 and 124); the frames' analyses are rebuilt with
    /// it. A change of `shared.profiles` starts pass 1 again: the returned
    /// view has `analysed = false` and the host drives
    /// [`Session::analyse_next`] to the end before the next edit.
    ///
    /// # Errors
    ///
    /// [`SessionError::Busy`] before pass 1 has finished and
    /// [`SessionError::Invalid`] when the shared settings fail validation.
    pub fn commit_roll(&mut self, roll: RollSettings) -> Result<RollView, SessionError> {
        self.check_analysed()?;
        roll.validate()
            .map_err(|e| SessionError::Invalid(e.to_string()))?;
        if roll != self.roll {
            let before = Arc::new(self.roll.clone());
            let hash = roll.content_hash();
            self.apply_roll(roll.clone());
            self.record(Edit::Roll {
                before,
                after: Arc::new(roll),
                hash,
            });
        }
        self.bump();
        Ok(self.roll_view())
    }

    /// Sets the roll's analysis area (Ruling 134), `shared.geometry.
    /// analysis_area`, at any time, before pass 1 too: the pre-analysis
    /// step of the app draws it on a frame's negative
    /// ([`Session::negative_tile`]) and then drives
    /// [`Session::analyse_next`]. Recorded in the history like a
    /// [`Session::commit_roll`] of the same roll settings with only the
    /// analysis area changed, and refreshed by the same rules (Ruling 81):
    /// frames pass 1 has not reached yet are analysed under it; a frame
    /// already analysed has its pool input rebuilt from its cached grid and
    /// the pool is marked pending. A frame whose overrides carry their own
    /// geometry keeps its own (the geometry is overridden as a whole,
    /// Ruling 92). `None` removes the area. Nothing is written until a
    /// flush.
    ///
    /// # Errors
    ///
    /// [`SessionError::Invalid`] when the area fails validation (the rules
    /// of a crop); nothing is changed then.
    pub fn set_roll_analysis_area(
        &mut self,
        area: Option<CropRect>,
    ) -> Result<RollView, SessionError> {
        let mut roll = self.roll.clone();
        roll.shared.geometry.analysis_area = area;
        roll.validate()
            .map_err(|e| SessionError::Invalid(e.to_string()))?;
        if roll != self.roll {
            let before = Arc::new(self.roll.clone());
            let hash = roll.content_hash();
            self.apply_roll(roll.clone());
            self.record(Edit::Roll {
                before,
                after: Arc::new(roll),
                hash,
            });
        }
        self.bump();
        Ok(self.roll_view())
    }

    /// Applies `preset` to the frame (ADR 0034): exactly
    /// [`Session::commit_frame`] with the frame's committed overrides after
    /// [`PrintLook::apply_to_overrides`], so only the five look fields and
    /// the preset label change.
    ///
    /// # Errors
    ///
    /// As [`Session::commit_frame`].
    pub fn apply_frame_preset(
        &mut self,
        frame: u32,
        preset: &Preset,
    ) -> Result<FrameView, SessionError> {
        let i = self.index(frame)?;
        let mut overrides = self.frames[i].overrides.clone();
        preset
            .look
            .apply_to_overrides(&preset.name, &mut overrides.print);
        self.commit_frame(frame, overrides)
    }

    /// The writable frames whose overrides set their own look (any look
    /// field or a frame preset), in index order: the frames a roll preset
    /// asks about (spec "Roll preset vs frames with their own look").
    pub fn frames_with_own_look(&self) -> Vec<u32> {
        self.frames
            .iter()
            .enumerate()
            .filter(|(_, f)| !f.read_only() && f.overrides.print.has_own_look())
            .map(|(i, _)| index_u32(i))
            .collect()
    }

    /// Applies `preset` to the roll (spec "Roll preset vs frames with
    /// their own look"): the roll's shared look is set with
    /// [`PrintLook::apply_to_settings`]; with
    /// [`RollPresetScope::AllFrames`] every writable frame with its own
    /// look also has its look fields cleared
    /// ([`negg_types::roll::PrintOverrides::clear_look`]), so it takes the
    /// roll's; its other overrides stay. With
    /// [`RollPresetScope::FramesWithoutOwnLook`] every writable frame with
    /// its own look has the whole look it renders with now pinned into its
    /// overrides (the five look fields and its resolved preset label;
    /// Ruling 5, a label of `None` pinned as `""`, Ruling 8), so it looks
    /// and is labelled exactly as before. A frame whose sidecar changed on
    /// disk turns read-only (as before any frame edit) and is left alone.
    /// The whole apply is one history entry; nothing is recorded when
    /// nothing changes. The changed frames and the roll are marked dirty,
    /// and the pool pending by the rules of a roll or frame commit.
    ///
    /// # Errors
    ///
    /// [`SessionError::Busy`] before pass 1 has finished and
    /// [`SessionError::Invalid`] when the roll or a cleared frame's
    /// settings fail validation; nothing is applied or recorded then (a
    /// frame found changed on disk has still turned read-only).
    pub fn apply_roll_preset(
        &mut self,
        preset: &Preset,
        scope: RollPresetScope,
    ) -> Result<RollView, SessionError> {
        self.check_analysed()?;
        let mut roll = self.roll.clone();
        preset
            .look
            .apply_to_settings(&preset.name, &mut roll.shared.print);
        roll.validate()
            .map_err(|e| SessionError::Invalid(e.to_string()))?;
        let mut frames = Vec::new();
        for i in 0..self.frames.len() {
            if self.frames[i].read_only() || !self.frames[i].overrides.print.has_own_look() {
                continue;
            }
            let path = negg_io::sidecar_path(&self.frames[i].path);
            if !self.frames[i].seen.unchanged(&path) {
                self.turn_stale(i, &path);
                continue;
            }
            let before = &self.frames[i].overrides;
            // Ruling 5 and 7: the rule `negg batch --preset` uses too.
            let after = preset_frame_overrides(&self.roll.shared.print, before, scope);
            if after == *before {
                continue;
            }
            validate(&resolve(&roll, &after))?;
            frames.push((i, Arc::new(before.clone()), Arc::new(after)));
        }
        if roll != self.roll || !frames.is_empty() {
            let roll_before = Arc::new(self.roll.clone());
            let hash = roll.content_hash();
            if roll != self.roll {
                self.apply_roll(roll.clone());
            }
            for (i, _, after) in &frames {
                self.frames[*i].preview = None;
                self.apply_frame(*i, (**after).clone(), false)?;
            }
            self.record(Edit::Preset {
                roll_before,
                roll_after: Arc::new(roll),
                frames,
                hash,
            });
        }
        self.bump();
        Ok(self.roll_view())
    }

    /// Opens frame `frame` for the pre-analysis step (Ruling 134): decodes
    /// it and builds its pyramid as [`Session::frame_view`] does (closing
    /// any other open frame), without pass 1, and returns its size and
    /// levels for [`Session::negative_tile`]. Loads the roll's profiles
    /// first when pass 1 has not, so the rig applies as it will in pass 1
    /// (when they cannot be loaded the negative is shown without the rig).
    ///
    /// # Errors
    ///
    /// [`SessionError::NoFrame`] for an index outside the roll,
    /// [`SessionError::Invalid`] for a frame that can never be converted,
    /// and [`SessionError::Convert`] when it cannot be decoded.
    pub fn negative_view(&mut self, frame: u32) -> Result<NegativeView, SessionError> {
        let i = self.index(frame)?;
        self.open_negative(i)?;
        let open = self.open.as_ref().expect("opened above");
        let level0 = &open.levels[0];
        Ok(NegativeView {
            index: frame,
            source: self.frames[i].source.clone(),
            width: index_u32(level0.width()),
            height: index_u32(level0.height()),
            levels: index_u32(open.levels.len()),
            tile_size: TILE_SIZE,
        })
    }

    /// One tile of the frame's decoded negative (Ruling 134), not inverted:
    /// level `level` of its pyramid, the tile at column `tile_x` and row
    /// `tile_y`, made viewable by `negg_render::negative_display` (one
    /// exposure scale for the frame, the step 7 matrix), turned to sRGB for
    /// display, in the tile wire format. In source orientation, uncropped,
    /// so the app draws the analysis area over it in source coordinates.
    /// Available before pass 1; the frame is opened when it is not.
    ///
    /// # Errors
    ///
    /// As [`Session::negative_view`], plus [`SessionError::Invalid`] for a
    /// level or tile outside the frame.
    pub fn negative_tile(
        &mut self,
        frame: u32,
        level: u32,
        tile_x: u32,
        tile_y: u32,
    ) -> Result<Vec<u8>, SessionError> {
        let i = self.index(frame)?;
        self.open_negative(i)?;
        let scale = self.negative_scale(i)?;
        self.ensure_srgb()?;
        let open = self.open.as_ref().expect("opened above");
        let image = open.levels.get(level as usize).ok_or_else(|| {
            SessionError::Invalid(format!("frame {frame} has no pyramid level {level}"))
        })?;
        let t = TILE_SIZE as usize;
        let (w, h) = (image.width(), image.height());
        let (x, y) = (tile_x as usize * t, tile_y as usize * t);
        if x >= w || y >= h {
            return Err(SessionError::Invalid(format!(
                "frame {frame} level {level} has no tile ({tile_x}, {tile_y})"
            )));
        }
        let (tw, th) = (t.min(w - x), t.min(h - y));
        let part = image.crop(x, y, tw, th)?;
        let shown = negative_display(&part, scale, open.camera_to_working);
        let rgb = self.srgb.as_ref().expect("built above").apply_u8(&shown);
        Ok(encode_tile(
            frame,
            self.generation,
            level,
            tile_x,
            tile_y,
            index_u32(tw),
            index_u32(th),
            &rgb_to_rgba(&rgb),
        ))
    }

    /// The frame's negative as a thumbnail (Ruling 134), for the
    /// pre-analysis filmstrip: the block medians of its analysis grid made
    /// viewable by `negg_render::negative_display`, not inverted, in source
    /// orientation and uncropped, turned to sRGB and encoded as a JPEG at
    /// [`THUMB_QUALITY`] with the sRGB profile. Available before pass 1:
    /// it uses pass 1's cached grid when there is one, else the grid an
    /// earlier negative view kept, else the open frame, else decodes the
    /// frame (without opening it) and keeps its grid as the frame's pass 1
    /// data (Ruling 140). Kept in memory until the roll's profiles change.
    ///
    /// # Errors
    ///
    /// [`SessionError::NoFrame`], [`SessionError::Invalid`] for a frame
    /// that can never be converted, and [`SessionError::Convert`] when it
    /// cannot be decoded or encoded.
    pub fn negative_thumbnail_jpeg(&mut self, frame: u32) -> Result<Vec<u8>, SessionError> {
        let i = self.index(frame)?;
        if let Some(jpeg) = &self.frames[i].negative_thumb {
            return Ok(jpeg.clone());
        }
        self.check_decodable(i)?;
        self.ensure_profiles();
        self.ensure_srgb()?;
        if self.frames[i].pass1.is_none()
            && self.frames[i].pre.is_none()
            && !self.open.as_ref().is_some_and(|o| o.index == i)
        {
            // Decoded once: kept as the frame's pass 1 data (Ruling 140)
            // when the profiles loaded, else used for this thumbnail only.
            let prepared = prepare_frame(&self.frames[i].path, self.rig())?;
            let pre = pass1_of(prepared);
            if self.keeps_pre(i) {
                self.frames[i].pre = Some(pre);
            } else {
                return self.encode_negative_thumbnail(i, &pre.grid, pre.camera_to_working);
            }
        }
        let (grid, camera) = match (&self.frames[i].pass1, &self.frames[i].pre, &self.open) {
            (Some(Ok(p)), _, _) | (_, Some(p), _) => (Cow::Borrowed(&p.grid), p.camera_to_working),
            (_, _, Some(o)) if o.index == i => (
                Cow::Owned(analysis_grid(&o.levels[0], ANALYSIS_GRID_LONG_SIDE)),
                o.camera_to_working,
            ),
            _ => unreachable!("decoded above"),
        };
        let grid = grid.into_owned();
        self.encode_negative_thumbnail(i, &grid, camera)
    }

    /// Encodes and keeps the negative thumbnail of frame `i` from its
    /// analysis `grid` (see [`Session::negative_thumbnail_jpeg`]).
    fn encode_negative_thumbnail(
        &mut self,
        i: usize,
        grid: &AnalysisGrid,
        camera: Option<[[f32; 3]; 3]>,
    ) -> Result<Vec<u8>, SessionError> {
        let scale = self.frames[i]
            .negative_scale
            .unwrap_or_else(|| negative_display_scale(grid));
        let shown = negative_display(&grid.median, scale, camera);
        let rgb = self.srgb.as_ref().expect("built above").apply_u8(&shown);
        let opts = JpegOptions {
            icc: Some(negg_cms::profile_bytes(OutputSpace::Srgb)?),
            quality: THUMB_QUALITY,
        };
        let jpeg = negg_io::encode_jpeg8(shown.width(), shown.height(), &rgb, &opts)?;
        let f = &mut self.frames[i];
        f.negative_scale = Some(scale);
        f.negative_thumb = Some(jpeg.clone());
        Ok(jpeg)
    }

    /// Lock (Ruling 89): the frame's current Dmin, gamma and cast estimates
    /// written into its overrides as Fixed ([`lock_overrides`], the
    /// function `negg lock` uses), committed as one edit. A pending re-pool
    /// runs first, so the locked values are those the export would use.
    ///
    /// # Errors
    ///
    /// As [`Session::commit_frame`], plus [`SessionError::Invalid`] when
    /// the frame has no estimates to lock.
    pub fn lock_frame(&mut self, frame: u32, which: LockWhich) -> Result<FrameView, SessionError> {
        let i = self.index(frame)?;
        self.check_editable(i)?;
        self.settle();
        let (_, plan) = self.plan(i, false)?;
        let locked = lock_overrides(&self.frames[i].overrides, &plan.estimates, which);
        self.commit_frame(frame, locked)
    }

    /// Undoes the last applied edit; a commit for sidecar purposes. Does
    /// nothing when there is none. Edits of a frame that turned read-only
    /// were dropped from the history when it did, so they are never in the
    /// way.
    ///
    /// # Errors
    ///
    /// [`SessionError::Busy`] before pass 1 has finished.
    pub fn undo(&mut self) -> Result<HistoryView, SessionError> {
        self.check_analysed()?;
        if self.applied == 0 {
            return Ok(self.history_view());
        }
        match self.history[self.applied - 1].clone() {
            Edit::Frame { frame, before, .. } => {
                self.frames[frame].preview = None;
                self.apply_frame(frame, (*before).clone(), false)?;
            }
            Edit::Roll { before, .. } => self.apply_roll((*before).clone()),
            Edit::Preset {
                roll_before,
                frames,
                ..
            } => {
                if *roll_before != self.roll {
                    self.apply_roll((*roll_before).clone());
                }
                for (i, before, _) in frames {
                    self.frames[i].preview = None;
                    self.apply_frame(i, (*before).clone(), false)?;
                }
            }
        }
        self.applied -= 1;
        self.bump();
        Ok(self.history_view())
    }

    /// Redoes the next undone edit; a commit for sidecar purposes. Does
    /// nothing when there is none.
    ///
    /// # Errors
    ///
    /// As [`Session::undo`].
    pub fn redo(&mut self) -> Result<HistoryView, SessionError> {
        self.check_analysed()?;
        if self.applied == self.history.len() {
            return Ok(self.history_view());
        }
        match self.history[self.applied].clone() {
            Edit::Frame {
                frame, after, hash, ..
            } => {
                self.frames[frame].preview = None;
                self.apply_frame(frame, (*after).clone(), false)?;
                debug_assert_eq!(resolve(&self.roll, &after).content_hash(), hash);
            }
            Edit::Roll { after, hash, .. } => {
                self.apply_roll((*after).clone());
                debug_assert_eq!(self.roll.content_hash(), hash);
            }
            Edit::Preset {
                roll_after,
                frames,
                hash,
                ..
            } => {
                for (i, _, after) in frames.iter().rev() {
                    self.frames[*i].preview = None;
                    self.apply_frame(*i, (**after).clone(), false)?;
                }
                if *roll_after != self.roll {
                    self.apply_roll((*roll_after).clone());
                }
                debug_assert_eq!(self.roll.content_hash(), hash);
            }
        }
        self.applied += 1;
        self.bump();
        Ok(self.history_view())
    }

    /// True when a flush has something to write for an edit: a commit
    /// changed a frame or the roll since the last flush, or a frame written
    /// before pass 1 reached it now has its estimates. (A flush may also
    /// bring an unedited frame's sidecar up to date, Ruling 139; that is
    /// found only by the flush itself.)
    pub fn is_dirty(&self) -> bool {
        let pooled = self.pooled();
        self.roll_dirty || self.frames.iter().any(|f| f.needs_write(pooled))
    }

    /// Writes the session's state to disk (Rulings 80, 122, 139 and 141).
    ///
    /// A pending re-pool runs first. Then every dirty frame's sidecar is
    /// written: the resolved settings, the overrides, `roll =
    /// "roll.negg.json"` and the estimates the frame renders with on the
    /// final pool (those `negg batch` would write). Until the pool is final
    /// (pass 1 restarted by a change of profiles has not finished) a
    /// sidecar is written without estimates, as an unanalysed frame's
    /// sidecar, and written again with them by the first flush after pass 1
    /// and its pool. A frame that failed pass 1 is written without
    /// estimates. Staleness is checked for every sidecar to write before
    /// any is written, so a frame found stale is pooled before estimates
    /// are written.
    ///
    /// Once the pool is final, every other writable frame that pass 1
    /// analysed is brought up to date too (Ruling 139): a roll edit (an
    /// analysis area, a crop, a pool flag) or another frame's edit can
    /// change its resolved settings or its estimates, so its sidecar is
    /// rewritten when it differs from the record the flush would write
    /// (the negg version that wrote it aside), exactly as `negg batch`
    /// would write it. This happens only when the session writes the roll
    /// at all (the roll has a `roll.negg.json`, or an edit was made), so a
    /// session that only browsed a folder never batched writes nothing
    /// (Ruling 141), and never while `roll.negg.json` changed behind the
    /// session: then only the frames edited in the session are written.
    ///
    /// Then the roll sidecar, when its settings, pool or frame list differ
    /// from `roll.negg.json` (or it is absent) and either an edit happened
    /// since the last flush, or the roll has a `roll.negg.json` that the
    /// final pool has made out of date (Ruling 141): the committed roll
    /// settings, the current pool when it has frames (else no pool), and
    /// the pool's frame list (else the frames `negg batch` would take). So
    /// committed roll settings are never lost, also while pass 1 runs again
    /// or when the pool failed.
    ///
    /// Every write is atomic. A read-only frame is never written. A
    /// sidecar that changed since the session last read or wrote it is not
    /// overwritten: its frame turns read-only (its overrides are read again
    /// from disk) and the error is returned after the other frames are
    /// written.
    ///
    /// # Errors
    ///
    /// [`SessionError::Stale`] for the first sidecar that changed on disk
    /// and [`SessionError::Convert`] for the first write that failed.
    pub fn flush(&mut self) -> Result<(), SessionError> {
        self.settle();
        let mut first: Option<SessionError> = None;
        let roll_path = negg_io::roll_sidecar_path(&self.dir);
        let roll_stale = !self.roll_seen.unchanged(&roll_path);
        // Whether the session writes the roll's sidecars at all (Ruling
        // 141): a folder never batched and only browsed gets none.
        let engaged = self.roll_dirty
            || self.roll_disk.is_some()
            || self.frames.iter().any(|f| f.dirty && !f.read_only());
        // Every sidecar to write is checked first, so that a frame found
        // stale (its overrides read again from disk) is in the pool before
        // any estimates are written.
        let pooled = self.pooled();
        let mut to_write = Vec::new();
        for i in 0..self.frames.len() {
            if self.frames[i].read_only() {
                self.frames[i].dirty = false;
                continue;
            }
            // Pass 1 has finished, so the pool is final after the settle
            // below (a frame found stale here makes it pending again).
            let refresh =
                engaged && !roll_stale && self.phase == Phase::Done && self.refreshable(i);
            if !self.frames[i].needs_write(pooled) && !refresh {
                continue;
            }
            let path = negg_io::sidecar_path(&self.frames[i].path);
            if self.frames[i].seen.unchanged(&path) {
                to_write.push((i, path));
            } else {
                self.turn_stale(i, &path);
                first.get_or_insert(SessionError::Stale(path));
            }
        }
        self.settle();
        let pooled = self.pooled();
        for (i, path) in to_write {
            if self.frames[i].read_only() {
                continue;
            }
            let edited = self.frames[i].needs_write(pooled);
            if !edited && !(pooled && self.refreshable(i)) {
                continue;
            }
            // Estimates only on the final pool: those `negg batch` writes.
            let estimates = if pooled {
                self.plan(i, false).ok().map(|(_, p)| p.estimates)
            } else {
                None
            };
            let f = &self.frames[i];
            let missing = estimates.is_none() && !pooled && !matches!(f.pass1, Some(Err(_)));
            let mut record = Sidecar::new(
                &f.path,
                resolve(&self.roll, &f.overrides),
                estimates,
                env!("CARGO_PKG_VERSION"),
            );
            record.overrides = f.overrides.clone();
            record.roll = Some(ROLL_SIDECAR_NAME.to_owned());
            if !edited && same_record(f.seen.bytes.as_deref(), &record) {
                continue;
            }
            let written = negg_io::sidecar_bytes(&record).and_then(|bytes| {
                negg_io::write_file_atomic(&path, &bytes).map(|meta| Seen::written(&meta, bytes))
            });
            match written {
                Ok(seen) => {
                    let f = &mut self.frames[i];
                    f.seen = seen;
                    f.dirty = false;
                    f.estimates_missing = missing;
                }
                Err(e) => {
                    first.get_or_insert(e.into());
                }
            }
        }
        if let Err(e) = self.flush_roll() {
            first.get_or_insert(e);
        }
        first.map_or(Ok(()), Err)
    }

    /// Whether a flush, on the final pool, brings frame `i`'s sidecar up
    /// to date although it was not edited (Ruling 139): the frame is
    /// writable and in the roll, and pass 1 analysed it, so `negg batch`
    /// would write its sidecar.
    fn refreshable(&self, i: usize) -> bool {
        let f = &self.frames[i];
        !f.read_only()
            && f.in_roll()
            && matches!(f.pass1, Some(Ok(_)))
            && matches!(f.analysis, Some(Ok(_)))
    }

    /// Exports by the CLI's own flows (Ruling 82).
    ///
    /// First every sidecar the session may write (each writable frame's
    /// and `roll.negg.json`) is checked against what the session last read
    /// or wrote (Ruling 122): a frame whose sidecar changed turns read-only
    /// with its overrides read again from disk, a changed roll sidecar is
    /// reported, and nothing is exported; the user sees what the export
    /// would use and can export again. Then the session flushes (running a
    /// pending re-pool), and the whole roll is `batch_dir` (`negg batch
    /// <dir> --format F [--out O] [--web | --web-only]`) and one or several
    /// frames are `convert_one` per frame (`negg convert <frame> --format F
    /// [--out O] [--web | --web-only]`), with no other options. The report
    /// lists the web copies too, each after its original. The export decodes and pools from disk
    /// and never reads the preview caches. Every flow event is passed on to
    /// `events`; a frame whose `convert_one` fails is reported as
    /// [`FlowEvent::FrameFailed`]. Afterwards the sidecars the flows
    /// rewrote are taken as seen, only for files that passed the check.
    ///
    /// # Errors
    ///
    /// [`SessionError::Busy`] before pass 1 has finished,
    /// [`SessionError::NoFrame`] for an index outside the roll,
    /// [`SessionError::Stale`] when a sidecar changed on disk, the errors
    /// of [`Session::flush`] (nothing is exported then), and
    /// [`SessionError::Convert`] when `batch_dir` fails as a whole.
    pub fn export(
        &mut self,
        req: &ExportRequest,
        events: &mut dyn FnMut(FlowEvent),
    ) -> Result<ExportReport, SessionError> {
        self.check_analysed()?;
        let frames = match &req.scope {
            ExportScope::Frame { frame } => Some(vec![self.index(*frame)?]),
            ExportScope::Frames { frames } => Some(
                frames
                    .iter()
                    .map(|f| self.index(*f))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            ExportScope::Roll => None,
        };
        self.check_disk()?;
        self.flush()?;
        // The flows decode every frame again at full size, so the open
        // frame (its full-size level 0 and pyramid) and the cached plans
        // are dropped first (Ruling 140): the export's peak is the flows'
        // own, plus the analysis grids the session keeps.
        self.open = None;
        for f in &mut self.frames {
            f.plans.clear();
        }
        // The files that passed the check (and the flush): only these are
        // taken as seen after the flows rewrite them.
        let checked: Vec<bool> = self.frames.iter().map(|f| !f.read_only()).collect();
        let out = OutputRequest {
            out_dir: req.out_dir.as_ref().map(PathBuf::from),
            format: match req.format {
                ExportFormat::Tiff16 => Format::Tiff16,
                ExportFormat::Jpeg => Format::Jpeg,
            },
            web: req.web,
            ..OutputRequest::default()
        };
        let mut report = ExportReport::default();
        let mut written: Vec<PathBuf> = Vec::new();
        let mut wrote_roll = false;
        let result = {
            let mut observe = |e: FlowEvent| {
                match &e {
                    FlowEvent::Converted {
                        input, converted, ..
                    } => {
                        report.converted += 1;
                        report
                            .outputs
                            .push(converted.output.to_string_lossy().into_owned());
                        if let Some(web) = &converted.web
                            && *web != converted.output
                        {
                            report.outputs.push(web.to_string_lossy().into_owned());
                        }
                        written.push(input.clone());
                    }
                    FlowEvent::FrameFailed { input, error } => {
                        report.failed += 1;
                        report.errors.push(format!("{}: {error}", input.display()));
                    }
                    FlowEvent::WroteRoll(_) => wrote_roll = true,
                    _ => {}
                }
                events(e);
            };
            match frames {
                None => batch_dir(&self.dir, &out, &mut observe).map(|_| ()),
                Some(list) => {
                    for i in list {
                        let input = self.frames[i].path.clone();
                        if let Err(error) = convert_one(&input, &out, &mut observe) {
                            observe(FlowEvent::FrameFailed { input, error });
                        }
                    }
                    Ok(())
                }
            }
        };
        for input in written {
            if let Some(i) = self.frames.iter().position(|f| f.path == input)
                && checked[i]
            {
                let f = &mut self.frames[i];
                f.seen = Seen::read(&negg_io::sidecar_path(&f.path));
                f.estimates_missing = false;
            }
        }
        if wrote_roll {
            let path = negg_io::roll_sidecar_path(&self.dir);
            self.roll_seen = Seen::read(&path);
            self.roll_disk = roll_from_bytes(&path, self.roll_seen.bytes.as_deref())
                .ok()
                .flatten();
        }
        result?;
        Ok(report)
    }

    /// The frame index for `frame`.
    fn index(&self, frame: u32) -> Result<usize, SessionError> {
        let i = frame as usize;
        if i < self.frames.len() {
            Ok(i)
        } else {
            Err(SessionError::NoFrame(frame))
        }
    }

    /// True when the roll's pool is final: pass 1 has finished and no
    /// re-pool is pending.
    fn pooled(&self) -> bool {
        self.phase == Phase::Done && !self.pool_pending
    }

    /// Edits wait for pass 1 (Ruling 81).
    fn check_analysed(&self) -> Result<(), SessionError> {
        if self.phase == Phase::Done {
            Ok(())
        } else {
            Err(SessionError::Busy)
        }
    }

    /// Frame edits also need a writable frame whose sidecar has not
    /// changed on disk behind the session: a stat (and, when the stamp
    /// moved, a read) before every edit, so that an edit is refused up
    /// front instead of being accepted and then dropped by the next flush.
    /// A changed sidecar turns the frame read-only (its overrides read
    /// again from disk) and is reported as [`SessionError::Stale`].
    fn check_editable(&mut self, i: usize) -> Result<(), SessionError> {
        self.check_analysed()?;
        if self.frames[i].read_only() {
            return Err(SessionError::ReadOnly(index_u32(i)));
        }
        let path = negg_io::sidecar_path(&self.frames[i].path);
        if !self.frames[i].seen.unchanged(&path) {
            self.turn_stale(i, &path);
            return Err(SessionError::Stale(path));
        }
        Ok(())
    }

    /// Checks every sidecar the session may write against what it last saw
    /// (Ruling 122). Changed frames turn read-only; the first change is
    /// returned.
    fn check_disk(&mut self) -> Result<(), SessionError> {
        let mut first = None;
        for i in 0..self.frames.len() {
            if self.frames[i].read_only() {
                continue;
            }
            let path = negg_io::sidecar_path(&self.frames[i].path);
            if !self.frames[i].seen.unchanged(&path) {
                self.turn_stale(i, &path);
                first.get_or_insert(SessionError::Stale(path));
            }
        }
        let path = negg_io::roll_sidecar_path(&self.dir);
        if !self.roll_seen.unchanged(&path) {
            first.get_or_insert(SessionError::Stale(path));
        }
        first.map_or(Ok(()), Err)
    }

    /// The frame's sidecar changed behind the session: the frame turns
    /// read-only, shows the overrides on disk (what an export uses; an
    /// unreadable file leaves it out of the roll, as `negg batch` does),
    /// its edits leave the history and the pool is marked pending.
    fn turn_stale(&mut self, i: usize, path: &Path) {
        let roll = self.roll.clone();
        let f = &mut self.frames[i];
        f.stale = Some(format!(
            "{} changed on disk since negg last read it; it is kept and the frame is read-only \
             until the roll is opened again",
            path.display()
        ));
        f.dirty = false;
        f.preview = None;
        f.estimates_missing = false;
        let old = resolve(&roll, &f.overrides);
        match read_overrides(&f.path) {
            Ok(o) => f.overrides = o,
            Err(e) => f.unreadable = Some(e.to_string()),
        }
        let new = resolve(&roll, &f.overrides);
        if area_changed(&old.geometry, &new.geometry) {
            f.analysis_stale = true;
        }
        if self.phase == Phase::Done {
            self.pool_pending = true;
        }
        self.drop_history_of(i);
        self.bump();
    }

    /// Removes every edit of frame `i` from the history (the frame turned
    /// read-only, so its edits can be neither undone nor redone), and its
    /// part of every roll preset edit (the roll part and other frames
    /// stay; a preset edit left with no change at all goes). Edits of other
    /// frames and of the roll do not depend on them.
    fn drop_history_of(&mut self, i: usize) {
        for e in &mut self.history {
            if let Edit::Preset { frames, .. } = e {
                frames.retain(|(frame, _, _)| *frame != i);
            }
        }
        let mut applied = self.applied;
        let mut k = 0;
        self.history.retain(|e| {
            let keep = match e {
                Edit::Frame { frame, .. } => *frame != i,
                Edit::Preset {
                    roll_before,
                    roll_after,
                    frames,
                    ..
                } => roll_before != roll_after || !frames.is_empty(),
                Edit::Roll { .. } => true,
            };
            if !keep && k < self.applied {
                applied -= 1;
            }
            k += 1;
            keep
        });
        self.applied = applied;
    }

    /// Advances the generation and returns it.
    fn bump(&mut self) -> u32 {
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }

    /// Records an applied edit: drops the redo tail, appends, keeps at most
    /// [`HISTORY_LIMIT`] (Ruling 79).
    fn record(&mut self, edit: Edit) {
        self.history.truncate(self.applied);
        self.history.push(edit);
        if self.history.len() > HISTORY_LIMIT {
            self.history.remove(0);
        }
        self.applied = self.history.len();
    }

    fn history_view(&self) -> HistoryView {
        HistoryView {
            can_undo: self.applied > 0,
            can_redo: self.applied < self.history.len(),
            len: index_u32(self.history.len()),
            cursor: index_u32(self.applied),
            pool_pending: self.pool_pending,
        }
    }

    /// Loads the roll's profiles when they are not loaded (the first
    /// `analyse_next` or a pre-analysis call); a failure is kept, and pass
    /// 1 then fails every frame with it.
    fn ensure_profiles(&mut self) {
        if self.profiles.is_none() {
            self.profiles = Some(match build_context(self.roll.clone(), &self.dir) {
                Ok((ctx, warnings)) => {
                    self.warnings = warnings;
                    Ok(Profiles {
                        rig: ctx.rig,
                        film: ctx.film,
                    })
                }
                Err(e) => Err(e.to_string()),
            });
        }
    }

    /// The rig in use, when the profiles loaded.
    fn rig(&self) -> Option<&RigCorrection> {
        match &self.profiles {
            Some(Ok(p)) => p.rig.as_ref(),
            _ => None,
        }
    }

    /// Whether a decode of frame `i` now is kept for pass 1 (Ruling 140):
    /// pass 1 has not reached it, nothing is kept yet, and the profiles
    /// loaded, so the decode used the rig pass 1 uses.
    fn keeps_pre(&self, i: usize) -> bool {
        let f = &self.frames[i];
        f.pass1.is_none() && f.pre.is_none() && matches!(self.profiles, Some(Ok(_)))
    }

    /// A frame `negg batch` refuses before pass 1 cannot be shown.
    fn check_decodable(&self, i: usize) -> Result<(), SessionError> {
        match &self.frames[i].blocked {
            Some(m) => Err(SessionError::Invalid(m.clone())),
            None => Ok(()),
        }
    }

    /// Opens the frame for its negative (Ruling 134): as
    /// [`Session::open_frame`] but without waiting for pass 1.
    fn open_negative(&mut self, i: usize) -> Result<(), SessionError> {
        self.check_decodable(i)?;
        self.ensure_profiles();
        self.decode_open(i)
    }

    /// The frame's negative display scale, worked out once from its
    /// analysis grid: pass 1's when there is one, else that of the open
    /// frame, which must be frame `i`.
    fn negative_scale(&mut self, i: usize) -> Result<f32, SessionError> {
        if let Some(scale) = self.frames[i].negative_scale {
            return Ok(scale);
        }
        let scale = match (&self.frames[i].pass1, &self.frames[i].pre, &self.open) {
            (Some(Ok(p)), _, _) | (_, Some(p), _) => negative_display_scale(&p.grid),
            (_, _, Some(o)) if o.index == i => {
                negative_display_scale(&analysis_grid(&o.levels[0], ANALYSIS_GRID_LONG_SIDE))
            }
            _ => return Err(SessionError::Busy),
        };
        self.frames[i].negative_scale = Some(scale);
        Ok(scale)
    }

    /// Whether the frame's Dmax in effect is pinned at the density cap
    /// (Ruling 133): its own analysed Dmax, or its fixed one, as the roll
    /// pool judges it. False before pass 1 reached it or when it failed.
    /// It reflects the last completed analysis: while a re-pool is pending
    /// (`pool_pending`) after an analysis area edit, a stale analysis is
    /// rebuilt only in the next `analyse_next`, so the flag may still show
    /// the old area until then.
    fn needs_analysis_area(&self, i: usize) -> bool {
        let f = &self.frames[i];
        let Some(Ok(analysis)) = &f.analysis else {
            return false;
        };
        let dmax = match resolve(&self.roll, &f.overrides).film.dmax {
            ChannelEstimate::Fixed { value } => value,
            ChannelEstimate::Auto => analysis.dmax,
        };
        dmax_pinned(&dmax)
    }

    /// The rig density model in use.
    fn density(&self) -> Option<&DensityModel> {
        match &self.profiles {
            Some(Ok(p)) => p.rig.as_ref().and_then(|r| r.density.as_ref()),
            _ => None,
        }
    }

    /// The film correction in use.
    fn film(&self) -> Option<&FilmCorrection> {
        match &self.profiles {
            Some(Ok(p)) => p.film.as_ref(),
            _ => None,
        }
    }

    /// The frame's pass 1 data: [`SessionError::Invalid`] when it failed
    /// or can never be converted, [`SessionError::Busy`] before pass 1
    /// reached it.
    fn pass1(&self, i: usize) -> Result<&Pass1, SessionError> {
        let f = &self.frames[i];
        match (&f.blocked, &f.pass1) {
            (Some(m), _) | (None, Some(Err(m))) => Err(SessionError::Invalid(m.clone())),
            (None, None) => Err(SessionError::Busy),
            (None, Some(Ok(p))) => Ok(p),
        }
    }

    /// The frame's step 7 matrix, once pass 1 has decoded it.
    fn camera(&self, i: usize) -> Option<[[f32; 3]; 3]> {
        self.pass1(i).ok().and_then(|p| p.camera_to_working)
    }

    /// The density of the frame's whole grid (with the rig density model):
    /// the pool input's own density when it was made without a crop, else
    /// rebuilt from the grid (Ruling 125).
    fn whole_density(&self, i: usize) -> Result<Cow<'_, Image>, SessionError> {
        self.pass1(i)?;
        Ok(frame_whole_density(&self.frames[i], self.density()).expect("pass 1 checked above"))
    }

    /// The settings the frame is drawn with (the preview when there is
    /// one).
    fn shown_settings(&self, i: usize) -> Settings {
        resolve(&self.roll, self.frames[i].shown())
    }

    /// Builds the display transform once.
    fn ensure_srgb(&mut self) -> Result<(), SessionError> {
        if self.srgb.is_none() {
            self.srgb = Some(negg_cms::working_to_output(
                OutputSpace::Srgb,
                Intent::Perceptual,
            )?);
        }
        Ok(())
    }

    /// The pooled values every plan uses now, as cache key bits.
    fn pool_key(&self) -> PoolKey {
        let p = self
            .pool
            .as_ref()
            .map(|p| p.pooled(&self.roll))
            .unwrap_or_default();
        let mut k = [0u32; 14];
        if let Some(d) = p.dmin {
            k[0] = 1;
            for c in 0..3 {
                k[1 + c] = d[c].to_bits();
            }
        }
        if let Some(g) = p.gamma {
            k[4] = 1;
            for c in 0..3 {
                k[5 + c] = g[c].to_bits();
            }
        }
        if let Some(c) = p.cast {
            k[8] = 1;
            k[9] = c.slope_r.to_bits();
            k[10] = c.offset_r.to_bits();
            k[11] = c.slope_b.to_bits();
            k[12] = c.offset_b.to_bits();
            k[13] = c.cells_used;
        }
        k
    }

    /// The frame's plan (Ruling 73) for its committed overrides, or the
    /// previewed ones when `shown`: [`plan_frame`] on the cached grid with
    /// the rig density model, the film correction and the pool filtered by
    /// the roll's flags, as `negg batch` renders it. Cached per pooled
    /// values and settings (Ruling 124).
    fn plan(&mut self, i: usize, shown: bool) -> Result<(PlanKey, FramePlan), SessionError> {
        let settings = if shown {
            self.shown_settings(i)
        } else {
            resolve(&self.roll, &self.frames[i].overrides)
        };
        let key = (self.pool_key(), settings.content_hash());
        self.pass1(i)?;
        if let Some((_, plan)) = self.frames[i].plans.iter().find(|(k, _)| *k == key) {
            return Ok((key, plan.clone()));
        }
        let plan = self.make_plan(i, &settings)?;
        let f = &mut self.frames[i];
        if f.plans.len() >= 2 {
            f.plans.remove(0);
        }
        f.plans.push((key, plan.clone()));
        Ok((key, plan))
    }

    /// Frame `i`'s plan for `settings`, not cached: [`plan_frame`] as
    /// [`Session::plan`] makes it.
    fn make_plan(&self, i: usize, settings: &Settings) -> Result<FramePlan, SessionError> {
        let pass1 = self.pass1(i)?;
        let whole = self.whole_density(i)?;
        let pooled = self.pool.as_ref().map(|p| p.pooled(&self.roll));
        let ctx = RenderContext {
            density: self.density(),
            film: self.film(),
            pooled: pooled.as_ref(),
        };
        Ok(plan_frame(&pass1.grid, &whole, settings, &ctx)?)
    }

    /// Decodes the frame again and builds its pyramid, unless it is the
    /// open frame. The previous open frame is dropped first (Ruling 87).
    fn open_frame(&mut self, i: usize) -> Result<(), SessionError> {
        // Checked first: a frame the negative path opened before pass 1 is
        // open already, but is not available to the converted views.
        self.pass1(i)?;
        self.decode_open(i)
    }

    /// Decodes frame `i` with the rig in use and makes it the open frame
    /// with its pyramid, unless it is open already. Before pass 1 the
    /// decode is also kept as the frame's pass 1 data (Ruling 140).
    fn decode_open(&mut self, i: usize) -> Result<(), SessionError> {
        if self.open.as_ref().is_some_and(|o| o.index == i) {
            return Ok(());
        }
        self.open = None;
        let prepared = prepare_frame(&self.frames[i].path, self.rig())?;
        if self.keeps_pre(i) {
            self.frames[i].pre = Some(Pass1 {
                grid: analysis_grid(&prepared.linear, ANALYSIS_GRID_LONG_SIDE),
                width: prepared.linear.width(),
                height: prepared.linear.height(),
                camera_to_working: prepared.camera_to_working,
                notes: prepared.notes.clone(),
            });
        }
        let level0 = prepared.linear;
        let upper = pyramid(&level0, TILE_SIZE as usize);
        let mut levels = Vec::with_capacity(upper.len() + 1);
        levels.push(level0);
        levels.extend(upper);
        self.open = Some(OpenFrame {
            index: i,
            levels,
            camera_to_working: prepared.camera_to_working,
        });
        Ok(())
    }

    /// The frame's view; does not open it.
    fn make_view(&mut self, i: usize) -> FrameView {
        let estimates = self.plan(i, false).ok().map(|(_, p)| p.estimates);
        let f = &self.frames[i];
        let (width, height) = match &f.pass1 {
            Some(Ok(p)) => (p.width, p.height),
            _ => (0, 0),
        };
        let levels = match &self.open {
            Some(o) if o.index == i => index_u32(o.levels.len()),
            _ => level_count(width, height),
        };
        let pool = self
            .pool
            .as_ref()
            .filter(|_| f.in_roll())
            .and_then(|p| p.frames.iter().find(|r| r.source == f.source))
            .cloned();
        FrameView {
            index: index_u32(i),
            source: f.source.clone(),
            width: index_u32(width),
            height: index_u32(height),
            levels,
            tile_size: TILE_SIZE,
            overrides: f.overrides.clone(),
            settings: resolve(&self.roll, &f.overrides),
            estimates,
            pool,
            generation: self.generation,
            read_only: f.read_only(),
            history: self.history_view(),
            pool_pending: self.pool_pending,
            needs_analysis_area: self.needs_analysis_area(i),
        }
    }

    /// The frame as the roll browser shows it.
    fn info(&self, i: usize) -> FrameInfo {
        let f = &self.frames[i];
        let (status, message) = if let Some(m) = &f.blocked {
            (FrameStatus::Failed, Some(m.clone()))
        } else if let Some(m) = f.unreadable.as_ref().or(f.stale.as_ref()) {
            (FrameStatus::ReadOnly, Some(m.clone()))
        } else {
            match (&f.pass1, &f.analysis) {
                (None, _) => (FrameStatus::Pending, None),
                (Some(Err(e)), _) | (Some(Ok(_)), Some(Err(e))) => {
                    (FrameStatus::Failed, Some(e.clone()))
                }
                (Some(Ok(p)), _) => (
                    FrameStatus::Ready,
                    (!p.notes.is_empty()).then(|| p.notes.join("; ")),
                ),
            }
        };
        FrameInfo {
            index: index_u32(i),
            source: f.source.clone(),
            status,
            message,
            needs_analysis_area: self.needs_analysis_area(i),
        }
    }

    /// The frame's pool input under `settings` (validation, then
    /// [`analyse_grid`] on its cached grid, as pass 1 of `negg batch`
    /// does) and whether it was made under a crop; `None` before pass 1.
    fn analysis_for(
        &self,
        i: usize,
        settings: &Settings,
    ) -> Option<(Result<FrameAnalysis, String>, bool)> {
        frame_analysis(&self.frames[i], settings, self.density())
    }

    /// Runs a pending re-pool: the analyses a roll edit made stale are
    /// rebuilt, then the roll is pooled.
    fn settle(&mut self) {
        if self.phase != Phase::Done || !self.pool_pending {
            return;
        }
        // The stale analyses are independent of one another, so they are
        // rebuilt on several threads; each result goes back to its frame.
        let jobs: Vec<(usize, Settings)> = (0..self.frames.len())
            .filter(|&i| self.frames[i].analysis_stale)
            .map(|i| (i, resolve(&self.roll, &self.frames[i].overrides)))
            .collect();
        if !jobs.is_empty() {
            let frames = &self.frames;
            let density = self.density();
            let workers = std::thread::available_parallelism()
                .map_or(1, usize::from)
                .min(jobs.len());
            let results: Vec<_> = std::thread::scope(|scope| {
                let handles: Vec<_> = jobs
                    .chunks(jobs.len().div_ceil(workers))
                    .map(|chunk| {
                        scope.spawn(move || {
                            chunk
                                .iter()
                                .map(|(i, settings)| {
                                    (*i, frame_analysis(&frames[*i], settings, density))
                                })
                                .collect::<Vec<_>>()
                        })
                    })
                    .collect();
                handles
                    .into_iter()
                    .flat_map(|h| h.join().expect("an analysis thread panicked"))
                    .collect()
            });
            for (i, result) in results {
                let f = &mut self.frames[i];
                if let Some((analysis, cropped)) = result {
                    f.analysis = Some(analysis);
                    f.analysis_cropped = cropped;
                }
                f.analysis_stale = false;
            }
        }
        self.repool();
    }

    /// Pools the analysed frames of the roll as `negg batch` does (the
    /// same frames, order, settings and film correction). The generation
    /// moves only when the pool changed.
    fn repool(&mut self) {
        let settings: Vec<Settings> = self
            .frames
            .iter()
            .map(|f| resolve(&self.roll, &f.overrides))
            .collect();
        let frames: Vec<RollFrame> = self
            .frames
            .iter()
            .zip(&settings)
            .filter(|(f, _)| f.in_roll())
            .filter_map(|(f, settings)| match &f.analysis {
                Some(Ok(analysis)) => Some(RollFrame {
                    source: &f.source,
                    analysis,
                    settings,
                }),
                _ => None,
            })
            .collect();
        let pooled = pool_roll(&frames, &self.roll, self.film());
        drop(frames);
        let (pool, error) = match pooled {
            Ok(pool) => (Some(pool), None),
            Err(e) => (None, Some(ConvertError::from(e).to_string())),
        };
        self.warn_if_border_lost(pool.as_ref());
        self.pool_pending = false;
        self.pool_error = error;
        if pool != self.pool {
            self.pool = pool;
            self.bump();
        }
    }

    /// Ruling 138: when an analysis area or crop change took the roll's
    /// Dmin pool from the clear film edge (Border frames) to percentile
    /// estimates, [`BORDER_LOST_WARNING`] is set (shown by
    /// [`Session::warnings`] and [`RollView::pool_warning`]); a pool back
    /// on the film edge clears it. The first pool of a session compares
    /// with the pool `roll.negg.json` stored, when the shared analysis
    /// rectangle differs from the one it was pooled with.
    fn warn_if_border_lost(&mut self, new: Option<&RollPool>) {
        let (before, moved) = match &self.pool {
            Some(p) => (Some(p), self.area_moved),
            None => match &self.roll_disk {
                Some(d) => (
                    d.pool.as_ref(),
                    !d.settings
                        .shared
                        .geometry
                        .same_analysis(&self.roll.shared.geometry),
                ),
                None => (None, false),
            },
        };
        let was_border = before.is_some_and(RollPool::dmin_from_border);
        let now_border = new.is_some_and(RollPool::dmin_from_border);
        if now_border {
            self.pool_warning = None;
        } else if moved && was_border {
            self.pool_warning = Some(BORDER_LOST_WARNING.to_owned());
        }
        self.area_moved = false;
    }

    /// Sets the frame's committed overrides and refreshes by Rulings 81 and
    /// 124: a change of the resolved crop or analysis buffer rebuilds its
    /// pool input from the cached grid; that, or a change of its resolved
    /// `film`, `colour.auto_colour` or `colour.cast`, marks the pool
    /// pending. With `strict` (a new commit) invalid settings or a crop too
    /// small to analyse are refused with nothing changed; an undo or redo
    /// applies them and the frame shows as failed.
    fn apply_frame(
        &mut self,
        i: usize,
        overrides: FrameOverrides,
        strict: bool,
    ) -> Result<(), SessionError> {
        let old = resolve(&self.roll, &self.frames[i].overrides);
        let new = resolve(&self.roll, &overrides);
        if strict {
            validate(&new)?;
        }
        let f = &self.frames[i];
        // A frame whose decode failed has nothing to analyse again, so its
        // edits never force a re-pool.
        let decoded = matches!(f.pass1, Some(Ok(_)));
        let moved = area_changed(&old.geometry, &new.geometry);
        let reanalyse =
            decoded && (moved || f.analysis_stale || matches!(f.analysis, Some(Err(_))));
        let analysis = if reanalyse {
            self.analysis_for(i, &new)
        } else {
            None
        };
        if strict
            && matches!(self.frames[i].pass1, Some(Ok(_)))
            && let Some((Err(e), _)) = &analysis
        {
            return Err(SessionError::Invalid(e.clone()));
        }
        let f = &mut self.frames[i];
        f.overrides = overrides;
        f.dirty = true;
        if reanalyse {
            if let Some((a, cropped)) = analysis {
                f.analysis = Some(a);
                f.analysis_cropped = cropped;
            }
            f.analysis_stale = false;
        }
        if f.in_roll() && decoded && (reanalyse || pool_inputs_changed(&old, &new)) {
            self.pool_pending = true;
            self.area_moved |= moved;
        }
        // Any frame edit makes the roll worth writing (Ruling 141): the
        // frame's sidecar names `roll.negg.json`.
        self.roll_dirty = true;
        Ok(())
    }

    /// Sets the committed roll settings and refreshes by Rulings 81 and
    /// 124: new profiles start pass 1 again; a frame whose resolved crop or
    /// buffer changed has its pool input marked for rebuilding; any of
    /// that, a pool flag, or a change of a frame's resolved `film`,
    /// `colour.auto_colour` or `colour.cast` marks the pool pending.
    fn apply_roll(&mut self, roll: RollSettings) {
        let old = std::mem::replace(&mut self.roll, roll);
        self.roll_dirty = true;
        if old.shared.profiles != self.roll.shared.profiles {
            self.restart_analysis();
            return;
        }
        let mut pool = old.pool_film_base != self.roll.pool_film_base
            || old.pool_gamma != self.roll.pool_gamma
            || old.pool_colour != self.roll.pool_colour;
        for f in &mut self.frames {
            // A frame pass 1 has not reached is analysed under the new
            // settings when it gets there; one whose decode failed has
            // nothing to analyse again.
            if !matches!(f.pass1, Some(Ok(_))) {
                continue;
            }
            let before = resolve(&old, &f.overrides);
            let after = resolve(&self.roll, &f.overrides);
            let moved = area_changed(&before.geometry, &after.geometry);
            if moved || matches!(f.analysis, Some(Err(_))) {
                f.analysis_stale = true;
                pool |= f.in_roll();
                self.area_moved |= moved && f.in_roll();
            }
            pool |= pool_inputs_changed(&before, &after) && f.in_roll();
        }
        if pool {
            self.pool_pending = true;
        }
    }

    /// Drops every analysis and the open frame and starts pass 1 again
    /// (new profiles change what every frame's analysis saw, Ruling 81).
    fn restart_analysis(&mut self) {
        self.phase = Phase::Frames;
        self.cursor = 0;
        self.profiles = None;
        self.warnings.clear();
        self.pool = None;
        self.pool_error = None;
        self.pool_pending = false;
        self.open = None;
        for f in &mut self.frames {
            f.pass1 = None;
            f.pre = None;
            f.analysis = None;
            f.analysis_cropped = false;
            f.analysis_stale = false;
            f.plans.clear();
            f.thumb = None;
            f.preset_thumbs.clear();
            f.negative_scale = None;
            f.negative_thumb = None;
        }
    }

    /// Writes the roll sidecar (see [`Session::flush`]); never over a file
    /// that changed behind the session.
    fn flush_roll(&mut self) -> Result<(), SessionError> {
        let pooled = self.pooled();
        let pool = self
            .pool
            .as_ref()
            .filter(|p| self.phase == Phase::Done && !p.frames.is_empty());
        let frames: Vec<String> = match pool {
            Some(p) => p.frames.iter().map(|f| f.source.clone()).collect(),
            None => self
                .frames
                .iter()
                .filter(|f| f.in_roll())
                .map(|f| f.source.clone())
                .collect(),
        };
        let same = self.roll_disk.as_ref().is_some_and(|d| {
            d.settings == self.roll && d.pool.as_ref() == pool && d.frames == frames
        });
        // Ruling 141: written for an edit, or when the final pool made an
        // existing roll sidecar out of date; never for browsing alone.
        let out_of_date = self.roll_disk.is_some() && pooled && pool.is_some();
        if same || (!self.roll_dirty && !out_of_date) {
            self.roll_dirty = false;
            return Ok(());
        }
        let path = negg_io::roll_sidecar_path(&self.dir);
        if !self.roll_seen.unchanged(&path) {
            return Err(SessionError::Stale(path));
        }
        let sidecar = RollSidecar {
            schema_version: ROLL_SIDECAR_SCHEMA_VERSION,
            settings_hash: self.roll.content_hash(),
            settings: self.roll.clone(),
            frames,
            pool: pool.cloned(),
            negg_version: env!("CARGO_PKG_VERSION").to_owned(),
        };
        let seen = negg_io::roll_sidecar_bytes(&sidecar)
            .and_then(|bytes| {
                negg_io::write_file_atomic(&path, &bytes).map(|meta| Seen::written(&meta, bytes))
            })
            .map_err(|source| ConvertError::RollSidecar {
                path: path.clone(),
                source,
            })?;
        self.roll_seen = seen;
        self.roll_disk = Some(sidecar);
        self.roll_dirty = false;
        Ok(())
    }
}

/// Whether the frame sidecar `on_disk` (its bytes as last seen) records
/// what `record` records, the negg version that wrote it aside: then a
/// refresh (Ruling 139) leaves it alone. A file that does not parse
/// differs.
fn same_record(on_disk: Option<&[u8]>, record: &Sidecar) -> bool {
    let Some(bytes) = on_disk else {
        return false;
    };
    let Ok(mut disk) = serde_json::from_slice(bytes)
        .map_err(negg_io::IoError::from)
        .and_then(negg_io::migrate)
    else {
        return false;
    };
    disk.negg_version.clone_from(&record.negg_version);
    disk == *record
}

/// The decode half of pass 1 for one frame: decode, prepare with the rig
/// of `profiles` and reduce to the whole-frame analysis grid.
fn decode_pass1(path: &Path, profiles: &Profiles) -> Result<Pass1, String> {
    let prepared = prepare_frame(path, profiles.rig.as_ref()).map_err(|e| e.to_string())?;
    Ok(pass1_of(prepared))
}

/// What pass 1 keeps of a prepared frame (Ruling 87): its whole-frame
/// analysis grid, its step 7 matrix, its size and the decoder's notes.
fn pass1_of(prepared: crate::convert::PreparedInput) -> Pass1 {
    Pass1 {
        grid: analysis_grid(&prepared.linear, ANALYSIS_GRID_LONG_SIDE),
        width: prepared.linear.width(),
        height: prepared.linear.height(),
        camera_to_working: prepared.camera_to_working,
        notes: prepared.notes,
    }
}

/// The analysis half of pass 1: the grid's density (with the rig density
/// model `density`), then the pool input under `settings`
/// ([`analyse_grid`]), as `analyse_frame` builds them. The grid density is
/// not kept: without a crop it lives on in the pool input (Ruling 125).
fn analyse_pass1(
    pass1: &Pass1,
    settings: &Settings,
    density: Option<&DensityModel>,
) -> Result<FrameAnalysis, String> {
    settings
        .validate()
        .and_then(|()| {
            let grid_density = density_grid(&pass1.grid.median, density);
            analyse_grid(
                &pass1.grid,
                Cow::Owned(grid_density),
                &settings.geometry,
                density,
            )
        })
        .map_err(|e| ConvertError::from(e).to_string())
}

/// The density of the frame's whole grid (with the rig density model
/// `density`): its pool input's own density when that was made without an
/// analysis area or a crop, else rebuilt from the grid (Ruling 125). `None` before pass 1 or
/// when it failed.
fn frame_whole_density<'a>(f: &'a Frame, density: Option<&DensityModel>) -> Option<Cow<'a, Image>> {
    let Some(Ok(pass1)) = &f.pass1 else {
        return None;
    };
    if let (Some(Ok(a)), false) = (&f.analysis, f.analysis_cropped) {
        return Some(Cow::Borrowed(&a.grid_density));
    }
    Some(Cow::Owned(density_grid(&pass1.grid.median, density)))
}

/// The frame's pool input under `settings` (validation, then
/// [`analyse_grid`] on its cached grid, as pass 1 of `negg batch` does)
/// and whether it was made under a crop; `None` before pass 1.
fn frame_analysis(
    f: &Frame,
    settings: &Settings,
    density: Option<&DensityModel>,
) -> Option<(Result<FrameAnalysis, String>, bool)> {
    match &f.pass1 {
        None => None,
        Some(Err(e)) => Some((Err(e.clone()), false)),
        Some(Ok(p)) => {
            let result = settings
                .validate()
                .and_then(|()| {
                    let whole = frame_whole_density(f, density).expect("pass 1 succeeded");
                    analyse_grid(&p.grid, whole, &settings.geometry, density)
                })
                .map_err(|e| ConvertError::from(e).to_string());
            let cropped = result.is_ok() && settings.geometry.analysis_rect().is_some();
            Some((result, cropped))
        }
    }
}

/// Whether the analysis area changed (Rulings 81 and 134): the analysis
/// rectangle (the analysis area, else the crop), or its buffer (without
/// either the buffer is ignored, Ruling 90).
fn area_changed(a: &Geometry, b: &Geometry) -> bool {
    !a.same_analysis(b)
}

/// Whether a frame's pool input settings changed (Ruling 81).
fn pool_inputs_changed(a: &Settings, b: &Settings) -> bool {
    a.film != b.film
        || a.colour.auto_colour != b.colour.auto_colour
        || a.colour.cast != b.colour.cast
}

/// Settings validation as a session error.
fn validate(settings: &Settings) -> Result<(), SessionError> {
    settings
        .validate()
        .map_err(|e| SessionError::Invalid(e.to_string()))
}

/// The geometry applied to a render (crop, rotate, flip; Ruling 70),
/// skipped for the identity.
fn apply_geometry(geometry: &Geometry, image: Image) -> Result<Image, SessionError> {
    if geometry.is_identity() {
        Ok(image)
    } else {
        Ok(geometry.apply(&image)?)
    }
}

/// Pyramid levels including level 0 for a `width` by `height` frame
/// (Ruling 73), by `negg_render::pyramid`'s own rule: halving rounds up, so
/// the level sizes of the long side alone decide when a level fits a tile,
/// and the pyramid of a one-row image of the long side has the same number
/// of levels.
fn level_count(width: usize, height: usize) -> u32 {
    let upper = pyramid(&Image::new(width.max(height), 1), TILE_SIZE as usize).len();
    index_u32(upper + 1)
}

/// RGB8 to RGBA8 with alpha 255 (a layout move, Ruling 70).
fn rgb_to_rgba(rgb: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgb.len() / 3 * 4);
    for px in rgb.as_chunks::<3>().0 {
        out.extend_from_slice(px);
        out.push(255);
    }
    out
}

/// A count or index as `u32` (frames, tiles and sizes always fit).
fn index_u32(i: usize) -> u32 {
    u32::try_from(i).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests;
