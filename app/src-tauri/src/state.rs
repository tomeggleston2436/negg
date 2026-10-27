// SPDX-License-Identifier: Apache-2.0

//! The app state behind every command ("### Tauri backend (P3-T)"): the open
//! [`Session`], the background analyse loop, the debounced sidecar flush and
//! the event sink. Nothing here touches a webview, so `cargo test` drives it
//! with a recording sink.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant};

use negg_session::{AnalyseStep, Gpu, RollView, Session, SessionError};
use serde::{Deserialize, Serialize};

/// The roll view as the app's commands send it: the session's [`RollView`]
/// (its keys flattened in) plus whether the roll waits for the user to
/// start the analysis (Ruling 136).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AppRollView {
    /// The session's view.
    #[serde(flatten)]
    pub view: RollView,
    /// The roll had no `roll.negg.json` when it was opened and the user has
    /// not pressed Analyse yet: pass 1 has not started. The app shows the
    /// negatives and the analysis area tool meanwhile.
    pub awaiting_analysis: bool,
}

impl std::ops::Deref for AppRollView {
    type Target = RollView;

    fn deref(&self) -> &RollView {
        &self.view
    }
}

/// Delay between the last commit and the sidecar flush (Ruling 80).
pub const FLUSH_DELAY: Duration = Duration::from_millis(500);

/// Event name of [`AnalysisEvent`].
pub const ANALYSIS_EVENT: &str = "negg://analysis";
/// Event name of [`ThumbsEvent`].
pub const THUMBS_EVENT: &str = "negg://thumbs";
/// Event name of [`ExportEvent`].
pub const EXPORT_EVENT: &str = "negg://export";
/// Event name of [`ErrorEvent`].
pub const ERROR_EVENT: &str = "negg://error";
/// Event name of the close handshake ([`Event::Closing`]): a close or quit
/// is about to flush; the webview leaves the field being typed in, sends
/// its queued edits and answers with the `close_ready` command.
pub const CLOSING_EVENT: &str = "negg://closing";

/// Progress of pass 1, or the end of a re-pool (`negg://analysis`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisEvent {
    /// Frames analysed so far.
    pub done: u32,
    /// Frames in the roll.
    pub total: u32,
    /// The frame just analysed; `None` for a pooling step.
    pub frame: Option<u32>,
    /// Pass 1 and the pool are finished; the host fetches its views again.
    pub finished: bool,
}

/// These frames' thumbnails are stale from `generation` on
/// (`negg://thumbs`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThumbsEvent {
    /// The frame indices.
    pub frames: Vec<u32>,
    /// The session generation the new thumbnails belong to.
    pub generation: u32,
}

/// One frame of an export finished (`negg://export`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportEvent {
    /// Frames finished so far.
    pub done: u32,
    /// Frames the export covers.
    pub total: u32,
    /// The source frame.
    pub input: String,
    /// Whether it was converted.
    pub ok: bool,
    /// The output written, or why it failed.
    pub message: Option<String>,
}

/// An error no command returned: a debounced flush that refused to
/// overwrite a sidecar, or a pool that could not be made (`negg://error`).
/// The host shows it and fetches its views again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorEvent {
    /// What went wrong, for the user.
    pub message: String,
}

/// One backend event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// `negg://analysis`.
    Analysis(AnalysisEvent),
    /// `negg://thumbs`.
    Thumbs(ThumbsEvent),
    /// `negg://export`.
    Export(ExportEvent),
    /// `negg://error`.
    Error(ErrorEvent),
    /// `negg://closing`, with an empty payload.
    Closing,
}

impl Event {
    /// The event name on the wire.
    pub fn name(&self) -> &'static str {
        match self {
            Event::Analysis(_) => ANALYSIS_EVENT,
            Event::Thumbs(_) => THUMBS_EVENT,
            Event::Export(_) => EXPORT_EVENT,
            Event::Error(_) => ERROR_EVENT,
            Event::Closing => CLOSING_EVENT,
        }
    }

    /// The payload as JSON.
    pub fn payload(&self) -> serde_json::Value {
        let v = match self {
            Event::Analysis(e) => serde_json::to_value(e),
            Event::Thumbs(e) => serde_json::to_value(e),
            Event::Export(e) => serde_json::to_value(e),
            Event::Error(e) => serde_json::to_value(e),
            Event::Closing => Ok(serde_json::json!({})),
        };
        v.unwrap_or(serde_json::Value::Null)
    }
}

/// Where events go: the webview in the app, a list in the tests.
pub trait EventSink: Send + Sync + 'static {
    /// Sends one event. Must not block.
    fn emit(&self, event: Event);
}

/// A command's error on the wire: `{ kind, message }`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandError {
    /// `convert`, `no_frame`, `read_only`, `busy`, `stale`, `invalid`,
    /// `no_roll`, `unsaved`, `exporting` or `internal`.
    pub kind: String,
    /// What went wrong, for the user.
    pub message: String,
}

impl CommandError {
    /// An error of `kind` with `message`.
    pub fn new(kind: &str, message: impl Into<String>) -> CommandError {
        CommandError {
            kind: kind.to_owned(),
            message: message.into(),
        }
    }

    /// No roll is open.
    pub fn no_roll() -> CommandError {
        CommandError::new("no_roll", "no roll is open")
    }

    /// A bug: a poisoned lock or a panicked worker.
    pub fn internal(message: impl Into<String>) -> CommandError {
        CommandError::new("internal", message)
    }
}

impl From<SessionError> for CommandError {
    fn from(e: SessionError) -> Self {
        let kind = match &e {
            SessionError::Convert(_) => "convert",
            SessionError::NoFrame(_) => "no_frame",
            SessionError::ReadOnly(_) => "read_only",
            SessionError::Busy => "busy",
            SessionError::Stale(_) => "stale",
            SessionError::Invalid(_) => "invalid",
        };
        CommandError::new(kind, e.to_string())
    }
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)
    }
}

impl std::error::Error for CommandError {}

/// The open session with the bookkeeping of its analyse loop.
struct Slot {
    session: Session,
    /// Identifies this session; a loop started for another one stops.
    id: u64,
    /// An analyse loop is running for this session.
    looping: bool,
    /// The analysis may run: the roll had a `roll.negg.json` when it was
    /// opened, or the user pressed Analyse (Ruling 136). Until then no
    /// edit starts the analyse loop.
    started: bool,
}

/// What the state shares with its worker threads.
struct Shared {
    slot: Mutex<Option<Slot>>,
    sink: Arc<dyn EventSink>,
    flush: Mutex<Option<Sender<()>>>,
    next_id: AtomicU64,
    /// A flush failed while opening another roll, closing the window or
    /// quitting, and the user was told; the next attempt goes ahead without
    /// the edits. Cleared by every edit, so new edits warn again.
    unsaved_warned: AtomicBool,
    /// An export is running (it holds the session for its whole length).
    exporting: AtomicBool,
    /// A close or quit is being prepared on a worker thread.
    closing: AtomicBool,
    /// The session was flushed for a close or quit, which may now go ahead.
    close_approved: AtomicBool,
    /// The webview answered the last `negg://closing` (its edits are sent).
    close_ack: Mutex<bool>,
    close_ack_cv: Condvar,
    /// The GPU preview device once [`AppState::start_gpu`] has looked for
    /// one (`None` inside: none found, or `NEGG_GPU=0`).
    gpu: OnceLock<Option<Arc<Gpu>>>,
}

/// Set to `0` to keep the preview on the CPU.
pub const GPU_ENV: &str = "NEGG_GPU";

/// How a command uses the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Views and previews: neither pass 1 nor the pool changes, so nothing
    /// is checked afterwards.
    Read,
    /// Edits: afterwards the analyse loop is started when pass 1 or a
    /// re-pool is outstanding (Ruling 124).
    Edit,
    /// Flush, lock and export, which settle a pending re-pool: it is made
    /// first, here, so the host hears of it (and of a pool error) as from
    /// the analyse loop; then as [`Access::Edit`].
    Settle,
}

/// The app state ("### Tauri backend (P3-T)"): the session behind a mutex,
/// the debounced flush and the event sink. Cloning shares it.
#[derive(Clone)]
pub struct AppState {
    shared: Arc<Shared>,
}

impl AppState {
    /// A state with no roll open, sending events to `sink`, with its
    /// debounced flush thread running.
    pub fn new(sink: Arc<dyn EventSink>) -> AppState {
        AppState::with_flush_delay(sink, FLUSH_DELAY)
    }

    /// As [`AppState::new`], flushing `delay` after the last edit instead of
    /// [`FLUSH_DELAY`]. A test that must flush by hand first uses a long one.
    pub fn with_flush_delay(sink: Arc<dyn EventSink>, delay: Duration) -> AppState {
        let shared = Arc::new(Shared {
            slot: Mutex::new(None),
            sink,
            flush: Mutex::new(None),
            next_id: AtomicU64::new(1),
            unsaved_warned: AtomicBool::new(false),
            exporting: AtomicBool::new(false),
            closing: AtomicBool::new(false),
            close_approved: AtomicBool::new(false),
            close_ack: Mutex::new(false),
            close_ack_cv: Condvar::new(),
            gpu: OnceLock::new(),
        });
        let (tx, rx) = mpsc::channel::<()>();
        *lock_ignoring_poison(&shared.flush) = Some(tx);
        let weak: Weak<Shared> = Arc::downgrade(&shared);
        std::thread::Builder::new()
            .name("negg-flush".into())
            .spawn(move || flush_thread(&weak, &rx, delay))
            .expect("the flush thread starts");
        AppState { shared }
    }

    /// Looks for a GPU for the preview (ADR 0036) on a background thread,
    /// unless [`GPU_ENV`] is `0`. Until it is found, and when there is none,
    /// tiles render on the CPU.
    pub fn start_gpu(&self) {
        if std::env::var(GPU_ENV).is_ok_and(|v| v == "0") {
            let _ = self.shared.gpu.set(None);
            return;
        }
        let shared = Arc::clone(&self.shared);
        let _ = std::thread::Builder::new()
            .name("negg-gpu".into())
            .spawn(move || {
                let gpu = match Gpu::new() {
                    Ok(g) => {
                        eprintln!("negg: preview on the GPU, {}", g.adapter());
                        Some(Arc::new(g))
                    }
                    Err(e) => {
                        eprintln!("negg: preview on the CPU, {e}");
                        None
                    }
                };
                let _ = shared.gpu.set(gpu);
            });
    }

    /// The GPU preview device, once found.
    pub fn gpu(&self) -> Option<Arc<Gpu>> {
        self.shared.gpu.get().cloned().flatten()
    }

    /// Sends one event.
    pub fn emit(&self, event: Event) {
        self.shared.sink.emit(event);
    }

    /// Runs `f` on the open session with `access` (see [`Access`]).
    pub fn with_session<R>(
        &self,
        access: Access,
        f: impl FnOnce(&mut Session) -> Result<R, SessionError>,
    ) -> Result<R, CommandError> {
        self.with_slot(access, |slot| f(&mut slot.session))
    }

    /// As [`AppState::with_session`] for a command answering with the roll
    /// view: whether the roll awaits Analyse is read under the same lock.
    pub fn with_roll(
        &self,
        access: Access,
        f: impl FnOnce(&mut Session) -> Result<RollView, SessionError>,
    ) -> Result<AppRollView, CommandError> {
        self.with_slot(access, |slot| {
            let view = f(&mut slot.session)?;
            Ok(AppRollView {
                view,
                awaiting_analysis: !slot.started,
            })
        })
    }

    fn with_slot<R>(
        &self,
        access: Access,
        f: impl FnOnce(&mut Slot) -> Result<R, SessionError>,
    ) -> Result<R, CommandError> {
        let mut events = Vec::new();
        let mut start = None;
        let result = {
            let mut guard = self.lock()?;
            let slot = guard.as_mut().ok_or_else(CommandError::no_roll)?;
            if access == Access::Read {
                f(slot)
            } else {
                if access == Access::Settle {
                    let before = slot.session.roll_view();
                    if before.analysed && before.pool_pending {
                        let step = slot.session.analyse_next();
                        if let Err(e) = step {
                            events.push(Event::Error(ErrorEvent {
                                message: e.to_string(),
                            }));
                        }
                        events.extend(pooled_events(&slot.session.roll_view()));
                    }
                }
                let result = f(slot);
                let after = slot.session.roll_view();
                if slot.started && needs_analysis(&after) && !slot.looping {
                    slot.looping = true;
                    start = Some(slot.id);
                }
                result
            }
        };
        for e in events {
            self.emit(e);
        }
        if let Some(id) = start {
            self.spawn_loop(id);
        }
        result.map_err(CommandError::from)
    }

    /// Runs `f` (an export) with the exporting flag set, so a close or quit
    /// meanwhile is refused with a message instead of waiting for it.
    pub fn exporting<R>(&self, f: impl FnOnce() -> R) -> R {
        struct Reset<'a>(&'a AtomicBool);
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::SeqCst);
            }
        }
        self.shared.exporting.store(true, Ordering::SeqCst);
        let _reset = Reset(&self.shared.exporting);
        f()
    }

    /// Opens the roll in `dir`, replacing the open session after flushing
    /// it. When the roll has a `roll.negg.json` (it was opened before) the
    /// analyse loop starts at once; otherwise the roll awaits Analyse
    /// ([`AppState::start_analysis`], Ruling 136) so the user can set the
    /// analysis area on the negatives first. When the old session's edits
    /// cannot be written, the old session stays open and the error says
    /// so; the next attempt opens the roll without them.
    pub fn open_roll(&self, dir: &Path) -> Result<AppRollView, CommandError> {
        let mut guard = match self.shared.slot.lock() {
            Ok(g) => g,
            Err(poisoned) => {
                // A command panicked inside the session: it is not flushed
                // (its state may be inconsistent) and is replaced.
                self.shared.slot.clear_poison();
                let mut g = poisoned.into_inner();
                *g = None;
                g
            }
        };
        if let Some(slot) = guard.as_mut() {
            match slot.session.flush() {
                Ok(()) => self.shared.unsaved_warned.store(false, Ordering::SeqCst),
                Err(e) => {
                    if !self.shared.unsaved_warned.swap(true, Ordering::SeqCst) {
                        return Err(CommandError::new(
                            "unsaved",
                            format!(
                                "the edits of the open roll could not be saved ({e}); \
                                 open the folder again to continue without them"
                            ),
                        ));
                    }
                }
            }
        }
        // Read before the session opens it: whether the roll was opened
        // before decides whether the analysis starts at once.
        let saved = negg_io::roll_sidecar_path(dir).exists();
        let session = Session::open_roll(dir)?;
        self.shared.unsaved_warned.store(false, Ordering::SeqCst);
        let view = session.roll_view();
        let id = self.shared.next_id.fetch_add(1, Ordering::SeqCst);
        *guard = Some(Slot {
            session,
            id,
            looping: saved,
            started: saved,
        });
        drop(guard);
        if saved {
            self.spawn_loop(id);
        }
        Ok(AppRollView {
            view,
            awaiting_analysis: !saved,
        })
    }

    /// Starts the analysis of a roll that awaits it (the Analyse button,
    /// Ruling 136): from now on the analyse loop runs whenever pass 1 or a
    /// re-pool is outstanding. Does nothing more when it has started.
    pub fn start_analysis(&self) -> Result<AppRollView, CommandError> {
        let (view, start) = {
            let mut guard = self.lock()?;
            let slot = guard.as_mut().ok_or_else(CommandError::no_roll)?;
            slot.started = true;
            let view = slot.session.roll_view();
            let start = needs_analysis(&view) && !slot.looping;
            if start {
                slot.looping = true;
            }
            (view, start.then_some(slot.id))
        };
        if let Some(id) = start {
            self.spawn_loop(id);
        }
        Ok(AppRollView {
            view,
            awaiting_analysis: false,
        })
    }

    /// Asks for a flush [`FLUSH_DELAY`] after the last call (Ruling 80).
    /// Called after every edit, so it also re-arms the unsaved-edits
    /// warning: a close after new edits warns again.
    pub fn schedule_flush(&self) {
        self.shared.unsaved_warned.store(false, Ordering::SeqCst);
        if let Some(tx) = lock_ignoring_poison(&self.shared.flush).as_ref() {
            let _ = tx.send(());
        }
    }

    /// Prepares a window close or app quit: flushes the session (blocking;
    /// the app calls it on a worker thread, never the main thread). A
    /// failed flush is returned once (the close is refused and the user is
    /// told); the next attempt goes ahead. While an export runs the close
    /// is refused at once, without waiting for it.
    ///
    /// # Errors
    ///
    /// Kind `exporting` while an export runs, `unsaved` when the flush
    /// failed for the first time since the last edit.
    pub fn prepare_close(&self) -> Result<(), CommandError> {
        if self.shared.exporting.load(Ordering::SeqCst) {
            return Err(CommandError::new(
                "exporting",
                "an export is running; close negg again when it has finished",
            ));
        }
        let result = match self.with_session(Access::Settle, Session::flush) {
            Err(e) if e.kind == "no_roll" => Ok(()),
            r => r,
        };
        match result {
            Ok(()) => Ok(()),
            Err(e) => {
                if self.shared.unsaved_warned.swap(true, Ordering::SeqCst) {
                    Ok(())
                } else {
                    Err(CommandError::new(
                        "unsaved",
                        format!(
                            "the edits could not be saved ({}); close or quit again to \
                             leave without them",
                            e.message
                        ),
                    ))
                }
            }
        }
    }

    /// The close handshake: sends `negg://closing` and waits up to `wait`
    /// for the webview's `close_ready` ([`AppState::close_ready`]), so the
    /// value being typed in a field (committed when the field is left) and
    /// every edit the webview queued reach the session before
    /// [`AppState::prepare_close`] flushes it. A webview that does not
    /// answer in time (gone, or hung) does not hold up the close. Returns
    /// whether it answered. Call it on a worker thread, never the main
    /// thread.
    pub fn await_frontend(&self, wait: Duration) -> bool {
        *lock_ignoring_poison(&self.shared.close_ack) = false;
        self.emit(Event::Closing);
        let deadline = Instant::now() + wait;
        let mut ack = lock_ignoring_poison(&self.shared.close_ack);
        while !*ack {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            ack = match self.shared.close_ack_cv.wait_timeout(ack, deadline - now) {
                Ok((guard, _)) => guard,
                Err(poisoned) => poisoned.into_inner().0,
            };
        }
        true
    }

    /// The webview's answer to `negg://closing`: its edits are sent.
    pub fn close_ready(&self) {
        *lock_ignoring_poison(&self.shared.close_ack) = true;
        self.shared.close_ack_cv.notify_all();
    }

    /// Starts preparing a close; false when one is already being prepared.
    pub fn begin_close(&self) -> bool {
        !self.shared.closing.swap(true, Ordering::SeqCst)
    }

    /// Ends the close [`AppState::begin_close`] started; when `approved`,
    /// the close or quit may go ahead without preparing again.
    pub fn end_close(&self, approved: bool) {
        if approved {
            self.shared.close_approved.store(true, Ordering::SeqCst);
        }
        self.shared.closing.store(false, Ordering::SeqCst);
    }

    /// True once a close or quit was prepared and may go ahead.
    pub fn close_approved(&self) -> bool {
        self.shared.close_approved.load(Ordering::SeqCst)
    }

    /// The last resort when the app exits without a prepared quit: flushes
    /// the session if it can be had within `wait`, ignoring errors (a
    /// refused sidecar stays as it is). Returns whether a flush ran.
    pub fn flush_on_exit(&self, wait: Duration) -> bool {
        let deadline = Instant::now() + wait;
        loop {
            match self.shared.slot.try_lock() {
                Ok(mut guard) => {
                    return match guard.as_mut() {
                        Some(slot) => {
                            let _ = slot.session.flush();
                            true
                        }
                        None => false,
                    };
                }
                Err(std::sync::TryLockError::Poisoned(_)) => return false,
                Err(std::sync::TryLockError::WouldBlock) => {
                    if Instant::now() >= deadline {
                        return false;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }

    /// True while an analyse loop runs for the open session (tests).
    pub fn is_analysing(&self) -> bool {
        self.lock()
            .ok()
            .and_then(|g| g.as_ref().map(|s| s.looping))
            .unwrap_or(false)
    }

    fn lock(&self) -> Result<MutexGuard<'_, Option<Slot>>, CommandError> {
        self.shared
            .slot
            .lock()
            .map_err(|_| CommandError::internal("the session failed earlier; open the roll again"))
    }

    fn spawn_loop(&self, id: u64) {
        let shared = Arc::clone(&self.shared);
        let spawned = std::thread::Builder::new()
            .name("negg-analyse".into())
            .spawn(move || analyse_loop(&shared, id));
        if let Err(e) = spawned {
            if let Ok(mut g) = self.shared.slot.lock()
                && let Some(slot) = g.as_mut()
                && slot.id == id
            {
                slot.looping = false;
            }
            self.emit(Event::Error(ErrorEvent {
                message: format!("could not start the analysis: {e}"),
            }));
        }
    }
}

/// Pass 1 has not finished, or a re-pool is pending.
fn needs_analysis(view: &RollView) -> bool {
    !view.analysed || view.pool_pending
}

/// The events after a pool was made: analysis finished, every thumbnail
/// stale.
fn pooled_events(view: &RollView) -> Vec<Event> {
    let total = u32::try_from(view.frames.len()).unwrap_or(u32::MAX);
    vec![
        Event::Analysis(AnalysisEvent {
            done: total,
            total,
            frame: None,
            finished: true,
        }),
        Event::Thumbs(ThumbsEvent {
            frames: (0..total).collect(),
            generation: view.generation,
        }),
    ]
}

/// The analyse loop of session `id`: lock, one `analyse_next`, unlock,
/// emit; until Done, or until another session replaces it.
fn analyse_loop(shared: &Shared, id: u64) {
    loop {
        let events = {
            let Ok(mut guard) = shared.slot.lock() else {
                return;
            };
            let Some(slot) = guard.as_mut() else {
                return;
            };
            if slot.id != id {
                return;
            }
            let step = slot.session.analyse_next();
            let view = slot.session.roll_view();
            let total = u32::try_from(view.frames.len()).unwrap_or(u32::MAX);
            match step {
                Ok(AnalyseStep::Done) => {
                    // Checked and cleared under the lock that edits take,
                    // so an edit either sees the loop running or starts one.
                    slot.looping = false;
                    return;
                }
                Ok(AnalyseStep::Analysed { frame, .. }) => vec![
                    Event::Analysis(AnalysisEvent {
                        done: (frame + 1).min(total),
                        total,
                        frame: Some(frame),
                        finished: false,
                    }),
                    Event::Thumbs(ThumbsEvent {
                        frames: vec![frame],
                        generation: view.generation,
                    }),
                ],
                Ok(AnalyseStep::Pooled) => pooled_events(&view),
                Err(e) => {
                    let mut v = vec![Event::Error(ErrorEvent {
                        message: e.to_string(),
                    })];
                    v.extend(pooled_events(&view));
                    v
                }
            }
        };
        for e in events {
            shared.sink.emit(e);
        }
    }
}

/// Waits for a quiet `delay` after the last request, then flushes.
fn flush_thread(shared: &Weak<Shared>, rx: &mpsc::Receiver<()>, delay: Duration) {
    while rx.recv().is_ok() {
        loop {
            match rx.recv_timeout(delay) {
                Ok(()) => {}
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        let Some(shared) = shared.upgrade() else {
            return;
        };
        let state = AppState { shared };
        match state.with_session(Access::Settle, Session::flush) {
            Ok(()) => {}
            Err(e) if e.kind == "no_roll" => {}
            Err(e) => state.emit(Event::Error(ErrorEvent {
                message: format!("could not save the edits: {}", e.message),
            })),
        }
    }
}

/// Locks a mutex whose data stays valid after a panic.
fn lock_ignoring_poison<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
