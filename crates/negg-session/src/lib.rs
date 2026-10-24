// SPDX-License-Identifier: Apache-2.0

//! The negg flows, shared by the `negg` CLI, the desktop app and the C ABI
//! (Ruling 68): settings-file parsing, decode and prepare, rig and film
//! loading, roll mode (pass 1 and pooling), one frame's conversion
//! ([`convert_frame_with`]), `negg convert` ([`convert_one`]), `negg batch`
//! ([`batch_dir`]) and `negg lock` ([`lock_frame`]).
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
pub mod roll;
pub mod settings_file;

pub use convert::{
    BatchSummary, Converted, Format, FrameOptions, PreparedInput, Timing, batch_inputs,
    check_targets, convert_frame, convert_frame_with, is_batch_input, output_path, prepare_input,
};
pub use error::ConvertError;
pub use flow::{FlowEvent, OutputRequest, batch_dir, convert_one};
pub use roll::{
    LockWhich, Locked, RollAnalysis, RollContext, analyse_roll, analyse_roll_frames, build_context,
    decode_linear, load_film, load_rig, lock_frame, lock_overrides, pool_is_current, prepare_frame,
    profile_path, read_overrides, render_in_roll, rig_mismatch,
};
pub use settings_file::{SettingsError, parse_settings, read_settings_file};
