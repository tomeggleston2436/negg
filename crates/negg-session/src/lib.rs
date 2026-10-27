// SPDX-License-Identifier: Apache-2.0

//! The negg flows, shared by the `negg` CLI, the desktop app and the C ABI
//! (Ruling 68): settings-file parsing, decode and prepare, rig and film
//! loading, roll mode (pass 1 and pooling), one frame's conversion
//! ([`convert_frame_with`]), `negg convert` ([`convert_one`]), `negg batch`
//! ([`batch_dir`]) and `negg lock` ([`lock_frame`]); and the [`Session`]
//! the desktop app and the C ABI drive (cached analysis, edit history,
//! preview tiles, thumbnails, scopes, sidecar flushing and export through
//! the same flows).
//!
//! The code was moved out of `negg-cli` with its behaviour unchanged.
//! Nothing here prints: what the CLI prints is reported through
//! [`FlowEvent`]s, warnings and notes are returned, and the CLI keeps the
//! printing with its `negg convert: ` and `negg batch: ` prefixes.
//!
//! Owns orchestration and I/O only. Must not contain image maths; that
//! lives in the engine crates.

pub mod convert;
pub mod error;
pub mod flow;
pub mod presets;
pub mod roll;
pub mod session;
pub mod settings_file;
pub mod view;
pub mod wire;

pub use convert::{
    BatchSummary, Converted, Format, FrameOptions, PreparedInput, Timing, WEB_JPEG_QUALITY,
    WEB_LONG_EDGE, WEB_SUBDIR, WebOutput, batch_inputs, check_targets, convert_frame,
    convert_frame_with, is_batch_input, output_path, prepare_input, web_output_path,
};
pub use error::{ConvertError, SessionError};
pub use flow::{FlowEvent, OutputRequest, batch_dir, convert_one};
/// The GPU device of the preview (feature `gpu`, ADR 0036).
#[cfg(feature = "gpu")]
pub use negg_render::gpu::{Gpu, GpuError};
pub use presets::{
    PRESETS_DIR_ENV, PresetEntry, PresetList, find_preset, list_preset_entries, list_presets,
    presets_dir, save_preset,
};
pub use roll::{
    LockWhich, Locked, RollAnalysis, RollContext, analyse_roll, analyse_roll_frames, build_context,
    decode_linear, load_film, load_rig, lock_frame, lock_overrides, pool_is_current, prepare_frame,
    profile_path, read_overrides, render_in_roll, rig_mismatch,
};
pub use session::{
    AnalyseStep, BORDER_LOST_WARNING, HISTORY_LIMIT, SCOPES_LONG_SIDE, Session, THUMB_QUALITY,
};
pub use settings_file::{
    SettingsError, SettingsFile, parse_settings, parse_settings_file, read_settings_file,
    read_settings_file_parts,
};
pub use view::{
    ExportFormat, ExportReport, ExportRequest, ExportScope, FrameInfo, FrameStatus, FrameView,
    HistoryView, NegativeView, RollPresetScope, RollView,
};
pub use wire::{
    SCOPES_HEADER_LEN, SCOPES_LEN, SCOPES_MAGIC, SCOPES_VERSION, TILE_HEADER_LEN, TILE_MAGIC,
    TILE_PIXEL_FORMAT_RGBA8, TILE_SIZE, TILE_VERSION, TileHeader, encode_scopes, encode_tile,
    parse_tile,
};
