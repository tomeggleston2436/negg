// SPDX-License-Identifier: Apache-2.0

//! The negg desktop app's backend (Ruling 67): Tauri commands over the
//! negg-session [`negg_session::Session`], linked as Rust crates (no C ABI).
//!
//! The command bodies live in [`commands`] and the state, the analyse loop,
//! the debounced flush and the events in [`state`]; the
//! `#[tauri::command]` wrappers here only forward, on a blocking thread so
//! that a long decode never stalls the webview. Arguments are snake_case
//! on the wire (Ruling 120).

pub mod commands;
pub mod state;

use std::sync::Arc;
use std::time::Duration;

use negg_session::{ExportReport, ExportRequest, FrameView, HistoryView, NegativeView};
use negg_types::{CropRect, FrameOverrides, RollSettings};
use tauri::ipc::Response;
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WindowEvent};

use crate::state::{AppRollView, AppState, CommandError, ErrorEvent, Event, EventSink};

/// Sends events to every webview of the app.
struct TauriSink(AppHandle);

impl EventSink for TauriSink {
    fn emit(&self, event: Event) {
        // A window that is gone cannot be told; nothing else to do.
        let _ = self.0.emit(event.name(), event.payload());
    }
}

/// Runs `f` on a blocking thread.
async fn blocking<T: Send + 'static>(
    state: &State<'_, AppState>,
    f: impl FnOnce(&AppState) -> Result<T, CommandError> + Send + 'static,
) -> Result<T, CommandError> {
    let st = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || f(&st))
        .await
        .map_err(|e| CommandError::internal(format!("the command failed: {e}")))?
}

#[tauri::command(rename_all = "snake_case")]
async fn open_roll(state: State<'_, AppState>, dir: String) -> Result<AppRollView, CommandError> {
    blocking(&state, move |s| commands::open_roll(s, dir)).await
}

#[tauri::command(rename_all = "snake_case")]
async fn start_analysis(state: State<'_, AppState>) -> Result<AppRollView, CommandError> {
    blocking(&state, commands::start_analysis).await
}

#[tauri::command(rename_all = "snake_case")]
async fn roll_view(state: State<'_, AppState>) -> Result<AppRollView, CommandError> {
    blocking(&state, commands::roll_view).await
}

#[tauri::command(rename_all = "snake_case")]
async fn negative_view(
    state: State<'_, AppState>,
    frame: u32,
) -> Result<NegativeView, CommandError> {
    blocking(&state, move |s| commands::negative_view(s, frame)).await
}

#[tauri::command(rename_all = "snake_case")]
async fn negative_tile(
    state: State<'_, AppState>,
    frame: u32,
    level: u32,
    tile_x: u32,
    tile_y: u32,
) -> Result<Response, CommandError> {
    blocking(&state, move |s| {
        commands::negative_tile(s, frame, level, tile_x, tile_y)
    })
    .await
    .map(Response::new)
}

#[tauri::command(rename_all = "snake_case")]
async fn negative_thumbnail(
    state: State<'_, AppState>,
    frame: u32,
) -> Result<Response, CommandError> {
    blocking(&state, move |s| commands::negative_thumbnail(s, frame))
        .await
        .map(Response::new)
}

#[tauri::command(rename_all = "snake_case")]
async fn set_roll_analysis_area(
    state: State<'_, AppState>,
    area: Option<CropRect>,
) -> Result<AppRollView, CommandError> {
    blocking(&state, move |s| commands::set_roll_analysis_area(s, area)).await
}

#[tauri::command(rename_all = "snake_case")]
async fn frame_view(state: State<'_, AppState>, frame: u32) -> Result<FrameView, CommandError> {
    blocking(&state, move |s| commands::frame_view(s, frame)).await
}

#[tauri::command(rename_all = "snake_case")]
async fn render_tile(
    state: State<'_, AppState>,
    frame: u32,
    level: u32,
    tile_x: u32,
    tile_y: u32,
) -> Result<Response, CommandError> {
    blocking(&state, move |s| {
        commands::render_tile(s, frame, level, tile_x, tile_y)
    })
    .await
    .map(Response::new)
}

#[tauri::command(rename_all = "snake_case")]
async fn thumbnail(state: State<'_, AppState>, frame: u32) -> Result<Response, CommandError> {
    blocking(&state, move |s| commands::thumbnail(s, frame))
        .await
        .map(Response::new)
}

#[tauri::command(rename_all = "snake_case")]
async fn scopes(state: State<'_, AppState>, frame: u32) -> Result<Response, CommandError> {
    blocking(&state, move |s| commands::scopes(s, frame))
        .await
        .map(Response::new)
}

#[tauri::command(rename_all = "snake_case")]
async fn preview_frame(
    state: State<'_, AppState>,
    frame: u32,
    overrides: FrameOverrides,
) -> Result<u32, CommandError> {
    blocking(&state, move |s| {
        commands::preview_frame(s, frame, overrides)
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
async fn commit_frame(
    state: State<'_, AppState>,
    frame: u32,
    overrides: FrameOverrides,
) -> Result<FrameView, CommandError> {
    blocking(&state, move |s| commands::commit_frame(s, frame, overrides)).await
}

#[tauri::command(rename_all = "snake_case")]
async fn commit_roll(
    state: State<'_, AppState>,
    roll: RollSettings,
) -> Result<AppRollView, CommandError> {
    blocking(&state, move |s| commands::commit_roll(s, roll)).await
}

#[tauri::command(rename_all = "snake_case")]
async fn lock_frame(
    state: State<'_, AppState>,
    frame: u32,
    dmin: bool,
    gamma: bool,
    cast: bool,
) -> Result<FrameView, CommandError> {
    blocking(&state, move |s| {
        commands::lock_frame(s, frame, dmin, gamma, cast)
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
async fn undo(state: State<'_, AppState>) -> Result<HistoryView, CommandError> {
    blocking(&state, commands::undo).await
}

#[tauri::command(rename_all = "snake_case")]
async fn redo(state: State<'_, AppState>) -> Result<HistoryView, CommandError> {
    blocking(&state, commands::redo).await
}

#[tauri::command(rename_all = "snake_case")]
fn close_ready(state: State<'_, AppState>) {
    commands::close_ready(&state);
}

#[tauri::command(rename_all = "snake_case")]
async fn flush(state: State<'_, AppState>) -> Result<(), CommandError> {
    blocking(&state, commands::flush).await
}

#[tauri::command(rename_all = "snake_case")]
async fn export(
    state: State<'_, AppState>,
    request: ExportRequest,
) -> Result<ExportReport, CommandError> {
    blocking(&state, move |s| commands::export(s, request)).await
}

/// The menu id of negg's own Quit item (macOS), which goes through the
/// prepared quit instead of terminating the app at once.
pub const QUIT_MENU_ID: &str = "negg-quit";
/// The menu ids of negg's own Undo and Redo items (macOS): they undo and
/// redo negg's edit history (the webview is told with `negg://menu`)
/// instead of the webview's text editing.
pub const UNDO_MENU_ID: &str = "negg-undo";
/// See [`UNDO_MENU_ID`].
pub const REDO_MENU_ID: &str = "negg-redo";
/// Event name of a menu command for the webview: `{ "action": "undo" }` or
/// `{ "action": "redo" }`.
pub const MENU_EVENT: &str = "negg://menu";

/// How long a close waits for the webview to send its pending edits
/// (`negg://closing`, then `close_ready`).
const CLOSE_ACK_WAIT: Duration = Duration::from_millis(1500);

/// How long the last-resort flush on exit waits for the session.
const EXIT_FLUSH_WAIT: Duration = Duration::from_secs(2);

/// What a prepared close ends with.
#[derive(Clone, Debug)]
enum CloseTarget {
    /// Destroy this window (the app then exits as its last window closed).
    Window(String),
    /// Exit the app.
    App,
}

/// Flushes on a worker thread (never the main thread: the flush may wait
/// for the session), then closes or exits; a refusal is sent as
/// `negg://error` and nothing closes. One close is prepared at a time.
fn request_close(app: &AppHandle, target: CloseTarget) {
    let state = app.state::<AppState>().inner().clone();
    if !state.begin_close() {
        return;
    }
    let app = app.clone();
    let worker = std::thread::Builder::new()
        .name("negg-close".into())
        .spawn({
            let state = state.clone();
            move || {
                // The webview first sends what it has pending (the field
                // being typed in, the queued edits), then the flush.
                state.await_frontend(CLOSE_ACK_WAIT);
                match state.prepare_close() {
                    Ok(()) => {
                        state.end_close(true);
                        match target {
                            CloseTarget::Window(label) => {
                                if let Some(w) = app.get_webview_window(&label) {
                                    let _ = w.destroy();
                                }
                            }
                            CloseTarget::App => app.exit(0),
                        }
                    }
                    Err(e) => {
                        state.end_close(false);
                        state.emit(Event::Error(ErrorEvent { message: e.message }));
                    }
                }
            }
        });
    if worker.is_err() {
        state.end_close(false);
        state.emit(Event::Error(ErrorEvent {
            message: "could not prepare the close; try again".to_owned(),
        }));
    }
}

/// The macOS menu: the standard items, with Quit replaced by negg's own
/// (Cmd+Q from the standard item terminates the app without any close or
/// exit request, so nothing could be flushed or refused), and Undo and Redo
/// replaced by negg's own, which undo and redo negg's edit history (the
/// standard items would undo the webview's text editing only).
#[cfg(target_os = "macos")]
fn app_menu(app: &AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder};
    let quit = MenuItemBuilder::with_id(QUIT_MENU_ID, "Quit negg")
        .accelerator("CmdOrCtrl+Q")
        .build(app)?;
    let negg = SubmenuBuilder::new(app, "negg")
        .about(None)
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .item(&quit)
        .build()?;
    let undo = MenuItemBuilder::with_id(UNDO_MENU_ID, "Undo")
        .accelerator("CmdOrCtrl+Z")
        .build(app)?;
    let redo = MenuItemBuilder::with_id(REDO_MENU_ID, "Redo")
        .accelerator("Shift+CmdOrCtrl+Z")
        .build(app)?;
    let edit = SubmenuBuilder::new(app, "Edit")
        .item(&undo)
        .item(&redo)
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;
    let window = SubmenuBuilder::new(app, "Window")
        .minimize()
        .maximize()
        .separator()
        .close_window()
        .build()?;
    MenuBuilder::new(app)
        .items(&[&negg, &edit, &window])
        .build()
}

/// Builds and runs the app.
///
/// # Panics
///
/// When the Tauri runtime cannot start (no display, a broken bundle).
pub fn run() {
    let builder = tauri::Builder::default();
    #[cfg(target_os = "macos")]
    let builder = builder.menu(app_menu);
    builder
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let sink: Arc<dyn EventSink> = Arc::new(TauriSink(app.handle().clone()));
            app.manage(AppState::new(sink));
            Ok(())
        })
        .on_menu_event(|app, event| {
            let id = event.id();
            if id == QUIT_MENU_ID {
                request_close(app, CloseTarget::App);
            } else if id == UNDO_MENU_ID || id == REDO_MENU_ID {
                let action = if id == UNDO_MENU_ID { "undo" } else { "redo" };
                let _ = app.emit(MENU_EVENT, serde_json::json!({ "action": action }));
            }
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                // Ruling 80: flush on close, off the main thread.
                if !window.state::<AppState>().close_approved() {
                    api.prevent_close();
                    request_close(
                        window.app_handle(),
                        CloseTarget::Window(window.label().to_owned()),
                    );
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            open_roll,
            start_analysis,
            roll_view,
            negative_view,
            negative_tile,
            negative_thumbnail,
            set_roll_analysis_area,
            frame_view,
            render_tile,
            thumbnail,
            scopes,
            preview_frame,
            commit_frame,
            commit_roll,
            lock_frame,
            undo,
            redo,
            close_ready,
            flush,
            export,
        ])
        .build(tauri::generate_context!())
        .expect("the negg app failed to start")
        .run(|app, event| match &event {
            RunEvent::ExitRequested { api, .. } => {
                if !app.state::<AppState>().close_approved() {
                    api.prevent_exit();
                    request_close(app, CloseTarget::App);
                }
            }
            RunEvent::Exit => {
                // Last resort (an exit nothing prepared): a best-effort
                // flush; a refused sidecar stays as it is.
                app.state::<AppState>().flush_on_exit(EXIT_FLUSH_WAIT);
            }
            _ => {}
        });
}
