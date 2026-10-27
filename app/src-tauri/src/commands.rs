// SPDX-License-Identifier: Apache-2.0

//! The command bodies ("### Tauri backend (P3-T)"): plain functions taking
//! [`AppState`], so `cargo test` runs them without a webview. The
//! `#[tauri::command]` wrappers in `lib.rs` only forward to these.
//!
//! Every edit (commit, lock, undo, redo) schedules the debounced flush
//! (Ruling 80) and, through [`AppState::with_session`], starts the analyse
//! loop when it left a re-pool pending (Ruling 124). Errors are never
//! retried by overwriting: a sidecar the session refuses stays as it is and
//! the error reaches the user (Rulings 80 and 122).

use std::path::{Path, PathBuf};

use negg_session::{
    ExportReport, ExportRequest, ExportScope, FlowEvent, FrameView, HistoryView, LockWhich,
    NegativeView, PresetList, RollPresetScope, RollView, Session, SessionError,
};
use negg_types::{CropRect, FrameOverrides, PrintLook, RollSettings};

use crate::state::{Access, AppRollView, AppState, CommandError, Event, ExportEvent, ThumbsEvent};

/// Opens the roll in `dir` (flushing and replacing any open one). Pass 1
/// starts in the background when the roll has a `roll.negg.json`; otherwise
/// the roll awaits [`start_analysis`] (Ruling 136).
pub fn open_roll(state: &AppState, dir: String) -> Result<AppRollView, CommandError> {
    state.open_roll(&PathBuf::from(dir))
}

/// Starts the analysis of a roll that awaits it (the Analyse button).
pub fn start_analysis(state: &AppState) -> Result<AppRollView, CommandError> {
    state.start_analysis()
}

/// The open roll.
pub fn roll_view(state: &AppState) -> Result<AppRollView, CommandError> {
    state.with_roll(Access::Read, |s| Ok(s.roll_view()))
}

/// Opens frame `frame` for its negative (Ruling 134): its size and
/// pyramid, before pass 1 too.
pub fn negative_view(state: &AppState, frame: u32) -> Result<NegativeView, CommandError> {
    state.with_session(Access::Read, |s| s.negative_view(frame))
}

/// One tile of the frame's negative in the tile wire format (the same
/// header as [`render_tile`]; the app keeps the two in separate caches).
pub fn negative_tile(
    state: &AppState,
    frame: u32,
    level: u32,
    tile_x: u32,
    tile_y: u32,
) -> Result<Vec<u8>, CommandError> {
    state.with_session(Access::Read, |s| {
        s.negative_tile(frame, level, tile_x, tile_y)
    })
}

/// The frame's negative thumbnail as JPEG bytes, before pass 1 too.
pub fn negative_thumbnail(state: &AppState, frame: u32) -> Result<Vec<u8>, CommandError> {
    state.with_session(Access::Read, |s| s.negative_thumbnail_jpeg(frame))
}

/// Sets the roll's analysis area (Ruling 134), at any time: before
/// Analyse it only records the area; after, the analyses it changes are
/// made again and the roll re-pooled in the background.
pub fn set_roll_analysis_area(
    state: &AppState,
    area: Option<CropRect>,
) -> Result<AppRollView, CommandError> {
    let view = state.with_roll(Access::Edit, |s| s.set_roll_analysis_area(area))?;
    edited(state, all_frames(&view), view.generation);
    Ok(view)
}

/// Opens frame `frame` and returns its view.
pub fn frame_view(state: &AppState, frame: u32) -> Result<FrameView, CommandError> {
    state.with_session(Access::Read, |s| s.frame_view(frame))
}

/// One preview tile in the tile wire format.
pub fn render_tile(
    state: &AppState,
    frame: u32,
    level: u32,
    tile_x: u32,
    tile_y: u32,
) -> Result<Vec<u8>, CommandError> {
    state.with_session(Access::Read, |s| {
        if !s.uses_gpu()
            && let Some(gpu) = state.gpu()
        {
            // Without the display matrix the tile renders on the CPU.
            let _ = s.set_gpu(Some(gpu));
        }
        s.render_tile(frame, level, tile_x, tile_y)
    })
}

/// The frame's thumbnail as JPEG bytes.
pub fn thumbnail(state: &AppState, frame: u32) -> Result<Vec<u8>, CommandError> {
    state.with_session(Access::Read, |s| s.thumbnail_jpeg(frame))
}

/// The frame's scopes in the scopes wire format.
pub fn scopes(state: &AppState, frame: u32) -> Result<Vec<u8>, CommandError> {
    state.with_session(Access::Read, |s| s.scopes(frame))
}

/// A drag step: no history, no sidecar. Returns the new generation.
pub fn preview_frame(
    state: &AppState,
    frame: u32,
    overrides: FrameOverrides,
) -> Result<u32, CommandError> {
    state.with_session(Access::Read, |s| s.preview_frame(frame, overrides))
}

/// Commits the frame's overrides.
pub fn commit_frame(
    state: &AppState,
    frame: u32,
    overrides: FrameOverrides,
) -> Result<FrameView, CommandError> {
    let view = state.with_session(Access::Edit, |s| s.commit_frame(frame, overrides))?;
    edited(state, vec![frame], view.generation);
    Ok(view)
}

/// Commits the roll settings.
pub fn commit_roll(state: &AppState, roll: RollSettings) -> Result<AppRollView, CommandError> {
    let view = state.with_roll(Access::Edit, |s| s.commit_roll(roll))?;
    edited(state, all_frames(&view), view.generation);
    Ok(view)
}

/// Locks the frame's estimates (all three when none is chosen).
pub fn lock_frame(
    state: &AppState,
    frame: u32,
    dmin: bool,
    gamma: bool,
    cast: bool,
) -> Result<FrameView, CommandError> {
    let which = LockWhich { dmin, gamma, cast };
    let view = state.with_session(Access::Settle, |s| s.lock_frame(frame, which))?;
    edited(state, vec![frame], view.generation);
    Ok(view)
}

/// The presets available: the built-ins and the user presets in the
/// presets folder (`NEGG_PRESETS_DIR`, else the config folder).
pub fn list_presets() -> PresetList {
    list_presets_in(negg_session::presets_dir().as_deref())
}

/// [`list_presets`] for the folder `dir`.
pub fn list_presets_in(dir: Option<&Path>) -> PresetList {
    negg_session::list_presets(dir)
}

/// Saves `look` as the user preset `name` and returns the refreshed list.
pub fn save_preset(name: &str, look: PrintLook) -> Result<PresetList, CommandError> {
    let dir = negg_session::presets_dir()
        .ok_or_else(|| CommandError::new("convert", "no presets folder is known"))?;
    save_preset_in(&dir, name, look)
}

/// [`save_preset`] into the folder `dir`.
pub fn save_preset_in(dir: &Path, name: &str, look: PrintLook) -> Result<PresetList, CommandError> {
    negg_session::save_preset(dir, name, &look)
        .map_err(|e| CommandError::from(SessionError::from(e)))?;
    Ok(negg_session::list_presets(Some(dir)))
}

/// Sets frame `frame`'s look from the preset `name`: one history entry.
pub fn apply_frame_preset(
    state: &AppState,
    frame: u32,
    name: &str,
) -> Result<FrameView, CommandError> {
    apply_frame_preset_in(state, negg_session::presets_dir().as_deref(), frame, name)
}

/// [`apply_frame_preset`] with presets from the folder `dir`.
pub fn apply_frame_preset_in(
    state: &AppState,
    dir: Option<&Path>,
    frame: u32,
    name: &str,
) -> Result<FrameView, CommandError> {
    let preset = find_preset(name, dir)?;
    let view = state.with_session(Access::Edit, |s| s.apply_frame_preset(frame, &preset))?;
    edited(state, vec![frame], view.generation);
    Ok(view)
}

/// The frames whose overrides set their own look: what a roll preset asks
/// the user about.
pub fn frames_with_own_look(state: &AppState) -> Result<Vec<u32>, CommandError> {
    state.with_session(Access::Read, |s| Ok(s.frames_with_own_look()))
}

/// Applies the preset `name` to the roll as one history entry; `scope` says
/// what happens to the frames with their own look.
pub fn apply_roll_preset(
    state: &AppState,
    name: &str,
    scope: RollPresetScope,
) -> Result<AppRollView, CommandError> {
    apply_roll_preset_in(state, negg_session::presets_dir().as_deref(), name, scope)
}

/// [`apply_roll_preset`] with presets from the folder `dir`.
pub fn apply_roll_preset_in(
    state: &AppState,
    dir: Option<&Path>,
    name: &str,
    scope: RollPresetScope,
) -> Result<AppRollView, CommandError> {
    let preset = find_preset(name, dir)?;
    let view = state.with_roll(Access::Edit, |s| s.apply_roll_preset(&preset, scope))?;
    edited(state, all_frames(&view), view.generation);
    Ok(view)
}

/// The frame as the preset `name` would print it, as JPEG bytes. Changes
/// nothing.
pub fn preset_thumbnail(state: &AppState, frame: u32, name: &str) -> Result<Vec<u8>, CommandError> {
    preset_thumbnail_in(state, negg_session::presets_dir().as_deref(), frame, name)
}

/// [`preset_thumbnail`] with presets from the folder `dir`.
pub fn preset_thumbnail_in(
    state: &AppState,
    dir: Option<&Path>,
    frame: u32,
    name: &str,
) -> Result<Vec<u8>, CommandError> {
    let preset = find_preset(name, dir)?;
    state.with_session(Access::Read, |s| {
        s.preset_thumbnail_jpeg(frame, &preset.look)
    })
}

fn find_preset(name: &str, dir: Option<&Path>) -> Result<negg_types::Preset, CommandError> {
    negg_session::find_preset(name, dir).map_err(|e| CommandError::from(SessionError::from(e)))
}

/// Undoes the last edit.
pub fn undo(state: &AppState) -> Result<HistoryView, CommandError> {
    history_step(state, Session::undo)
}

/// Redoes the next undone edit.
pub fn redo(state: &AppState) -> Result<HistoryView, CommandError> {
    history_step(state, Session::redo)
}

/// The webview's answer to `negg://closing` (the close handshake): every
/// edit it had pending has been sent.
pub fn close_ready(state: &AppState) {
    state.close_ready();
}

/// Writes the session's sidecars now.
pub fn flush(state: &AppState) -> Result<(), CommandError> {
    state.with_session(Access::Settle, Session::flush)
}

/// Exports by the CLI's flows (Ruling 82), sending `negg://export` per
/// frame. The session flushes first; a sidecar changed by another program
/// stops the export with that error and nothing is overwritten.
pub fn export(state: &AppState, request: ExportRequest) -> Result<ExportReport, CommandError> {
    let sink = state.clone();
    state.exporting(|| {
        state.with_session(Access::Settle, |s| {
            let total = match &request.scope {
                ExportScope::Frame { .. } => 1,
                ExportScope::Frames { frames } => u32::try_from(frames.len()).unwrap_or(u32::MAX),
                ExportScope::Roll => s.frame_count(),
            };
            let mut done = 0u32;
            let mut on_event = |e: FlowEvent| {
                let (input, ok, message) = match e {
                    FlowEvent::Converted {
                        input, converted, ..
                    } => (
                        input,
                        true,
                        Some(converted.output.to_string_lossy().into_owned()),
                    ),
                    FlowEvent::FrameFailed { input, error } => {
                        (input, false, Some(error.to_string()))
                    }
                    _ => return,
                };
                done = done.saturating_add(1).min(total.max(1));
                sink.emit(Event::Export(ExportEvent {
                    done,
                    total,
                    input: input.to_string_lossy().into_owned(),
                    ok,
                    message,
                }));
                // Lets the event loop deliver the progress between frames.
                std::thread::yield_now();
            };
            s.export(&request, &mut on_event)
        })
    })
}

/// Undo or redo: either may change the roll or any frame, so every
/// thumbnail is stale.
fn history_step(
    state: &AppState,
    step: fn(&mut Session) -> Result<HistoryView, negg_session::SessionError>,
) -> Result<HistoryView, CommandError> {
    let (history, view) = state.with_session(Access::Edit, |s| {
        let h = step(s)?;
        Ok((h, s.roll_view()))
    })?;
    edited(state, all_frames(&view), view.generation);
    Ok(history)
}

/// After an edit: these thumbnails are stale and a flush is due.
fn edited(state: &AppState, frames: Vec<u32>, generation: u32) {
    state.emit(Event::Thumbs(ThumbsEvent { frames, generation }));
    state.schedule_flush();
}

fn all_frames(view: &RollView) -> Vec<u32> {
    (0..u32::try_from(view.frames.len()).unwrap_or(u32::MAX)).collect()
}
