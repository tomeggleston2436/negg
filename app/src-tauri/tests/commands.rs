// SPDX-License-Identifier: Apache-2.0

//! The command bodies driven without a webview (Ruling 84) on synthetic
//! rolls in a scratch directory: the analyse loop and its events, pending
//! re-pools after edits (Ruling 124), the debounced flush, data safety
//! (Rulings 80 and 122) and export.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use negg_app_lib::commands;
use negg_app_lib::state::{AppState, CommandError, Event, EventSink, FLUSH_DELAY};
use negg_session::{
    ExportFormat, ExportRequest, ExportScope, FrameStatus, SCOPES_LEN, TILE_HEADER_LEN, parse_tile,
};
use negg_types::settings::ChannelEstimate;
use negg_types::{CropRect, FrameOverrides, Geometry};

/// Records every event.
#[derive(Default)]
struct Recorder(Mutex<Vec<Event>>);

impl EventSink for Recorder {
    fn emit(&self, event: Event) {
        self.0.lock().unwrap().push(event);
    }
}

impl Recorder {
    fn take(&self) -> Vec<Event> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

/// A scratch directory removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        // The counter keeps names unique when tests start within the
        // clock's resolution (microseconds on macOS).
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("negg-app-{tag}-{}-{nanos}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn dir(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A synthetic negative as an untagged 16-bit TIFF: a clear border of
/// density `dmin` around a textured ramp.
fn write_negative(path: &Path, w: usize, h: usize, dmin: [f32; 3]) {
    let border = (w.min(h) / 10).max(2);
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

fn write_roll(dir: &Scratch, n: usize) {
    for i in 0..n {
        let d = 0.01 * i as f32;
        write_negative(
            &dir.path(&format!("f{i}.tif")),
            96,
            64,
            [0.25 + d, 0.55 + d, 0.75 + d],
        );
    }
}

/// Every file name in `dir` with its bytes.
fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            (
                e.file_name().to_string_lossy().into_owned(),
                std::fs::read(e.path()).unwrap(),
            )
        })
        .collect();
    v.sort();
    v
}

fn state() -> (AppState, Arc<Recorder>) {
    let rec = Arc::new(Recorder::default());
    (AppState::new(rec.clone()), rec)
}

/// Waits until pass 1 and any pending re-pool are finished.
fn wait_idle(state: &AppState) {
    let start = Instant::now();
    loop {
        let v = commands::roll_view(state).unwrap();
        if v.analysed && !v.pool_pending && !state.is_analysing() {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(120),
            "analysis did not finish"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn grade(g: f32) -> FrameOverrides {
    let mut o = FrameOverrides::default();
    o.print.grade = Some(g);
    o
}

fn finished(events: &[Event]) -> bool {
    events
        .iter()
        .any(|e| matches!(e, Event::Analysis(a) if a.finished && a.done == a.total))
}

#[test]
fn no_roll_is_an_error() {
    let (st, _) = state();
    let e = commands::roll_view(&st).unwrap_err();
    assert_eq!(e.kind, "no_roll");
    assert_eq!(
        serde_json::to_string(&e).unwrap(),
        r#"{"kind":"no_roll","message":"no roll is open"}"#
    );
}

#[test]
fn open_runs_analysis_in_the_background_and_writes_nothing() {
    let dir = Scratch::new("open");
    write_roll(&dir, 3);
    let before = snapshot(&dir.0);
    let (st, rec) = state();
    let view = commands::open_roll(&st, dir.dir()).unwrap();
    assert_eq!(view.frames.len(), 3);
    // No roll.negg.json: the roll waits for Analyse (Ruling 136).
    assert!(view.awaiting_analysis);
    assert!(!st.is_analysing());
    let started = commands::start_analysis(&st).unwrap();
    assert!(!started.awaiting_analysis);
    wait_idle(&st);
    let events = rec.take();
    let analysed: Vec<u32> = events
        .iter()
        .filter_map(|e| match e {
            Event::Analysis(a) if !a.finished => a.frame,
            _ => None,
        })
        .collect();
    assert_eq!(analysed, vec![0, 1, 2]);
    assert!(finished(&events), "{events:?}");
    let thumbs_all = events
        .iter()
        .any(|e| matches!(e, Event::Thumbs(t) if t.frames == vec![0, 1, 2] && t.generation > 0));
    assert!(thumbs_all, "{events:?}");
    let view = commands::roll_view(&st).unwrap();
    assert!(view.frames.iter().all(|f| f.status == FrameStatus::Ready));

    // Previews: a tile, a thumbnail, the scopes.
    let fv = commands::frame_view(&st, 1).unwrap();
    assert_eq!((fv.width, fv.height), (96, 64));
    let tile = commands::render_tile(&st, 1, 0, 0, 0).unwrap();
    let (header, _) = parse_tile(&tile).unwrap();
    assert_eq!((header.frame, header.width, header.height), (1, 96, 64));
    assert_eq!(tile.len(), TILE_HEADER_LEN + 96 * 64 * 4);
    let jpeg = commands::thumbnail(&st, 0).unwrap();
    assert_eq!(&jpeg[..2], &[0xff, 0xd8]);
    assert_eq!(commands::scopes(&st, 1).unwrap().len(), SCOPES_LEN);
    let g = commands::preview_frame(&st, 1, grade(130.0)).unwrap();
    assert!(g > fv.generation);
    assert_eq!(snapshot(&dir.0), before, "viewing wrote something");
}

#[test]
fn edits_flush_after_the_debounce_and_repools_run_in_the_background() {
    let dir = Scratch::new("edit");
    write_roll(&dir, 3);
    let (st, rec) = state();
    commands::open_roll(&st, dir.dir()).unwrap();
    commands::start_analysis(&st).unwrap();
    wait_idle(&st);
    rec.take();

    // A grade change leaves the pool; the sidecar follows the debounce.
    let v = commands::commit_frame(&st, 0, grade(125.0)).unwrap();
    assert!(!v.pool_pending);
    assert!(v.history.can_undo);
    let sidecar = dir.path("f0.negg.json");
    assert!(!sidecar.exists(), "written before the debounce");
    let start = Instant::now();
    while !sidecar.exists() {
        assert!(start.elapsed() < FLUSH_DELAY * 20, "never flushed");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        negg_session::read_overrides(&dir.path("f0.tif")).unwrap(),
        grade(125.0)
    );
    let events = rec.take();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::Thumbs(t) if t.frames == vec![0]))
    );

    // Locking a frame's film base changes what the pool sees: the commit
    // returns at once with the re-pool pending, the loop makes it.
    let mut o = grade(125.0);
    o.film.film_base = Some(ChannelEstimate::Fixed {
        value: [0.3, 0.6, 0.8],
    });
    let v = commands::commit_frame(&st, 1, o).unwrap();
    assert!(v.pool_pending);
    wait_idle(&st);
    let events = rec.take();
    assert!(finished(&events), "{events:?}");
    let after = commands::frame_view(&st, 1).unwrap();
    assert!(!after.pool_pending);
    assert!(after.generation > v.generation);

    // Undo and redo report through the history view and repool too.
    let h = commands::undo(&st).unwrap();
    assert!(h.can_redo);
    wait_idle(&st);
    let h = commands::redo(&st).unwrap();
    assert!(!h.can_redo);
    wait_idle(&st);

    // Lock: the frame's estimates become fixed overrides.
    let locked = commands::lock_frame(&st, 2, true, false, false).unwrap();
    assert!(matches!(
        locked.overrides.film.film_base,
        Some(ChannelEstimate::Fixed { .. })
    ));
    wait_idle(&st);
    commands::flush(&st).unwrap();
    assert!(dir.path("f2.negg.json").exists());
    assert!(dir.path("roll.negg.json").exists());
}

#[test]
fn a_sidecar_changed_by_another_program_is_never_overwritten() {
    let dir = Scratch::new("stale");
    write_roll(&dir, 2);
    let (st, _rec) = state();
    commands::open_roll(&st, dir.dir()).unwrap();
    commands::start_analysis(&st).unwrap();
    wait_idle(&st);
    commands::commit_frame(&st, 0, grade(120.0)).unwrap();
    commands::flush(&st).unwrap();
    let sidecar = dir.path("f0.negg.json");

    // A commit, then another program rewrites the sidecar with other
    // bytes before the flush.
    commands::commit_frame(&st, 0, grade(130.0)).unwrap();
    let foreign = std::fs::read_to_string(&sidecar)
        .unwrap()
        .replace("120.0", "99.0");
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(&sidecar, &foreign).unwrap();

    let e = commands::flush(&st).unwrap_err();
    assert_eq!(e.kind, "stale", "{e:?}");
    assert_eq!(std::fs::read_to_string(&sidecar).unwrap(), foreign);
    let v = commands::roll_view(&st).unwrap();
    assert_eq!(v.frames[0].status, FrameStatus::ReadOnly);
    let e = commands::commit_frame(&st, 0, grade(140.0)).unwrap_err();
    assert_eq!(e.kind, "read_only");
    assert!(e.message.starts_with("frame 1 is read-only"), "{e:?}");
    commands::flush(&st).unwrap();
    assert_eq!(std::fs::read_to_string(&sidecar).unwrap(), foreign);

    // An edit of a frame whose sidecar changed before it is refused up
    // front, so it is never accepted and then dropped.
    commands::commit_frame(&st, 1, grade(110.0)).unwrap();
    commands::flush(&st).unwrap();
    let one = dir.path("f1.negg.json");
    let foreign_one = std::fs::read_to_string(&one)
        .unwrap()
        .replace("110.0", "90.0");
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(&one, &foreign_one).unwrap();
    let e = commands::commit_frame(&st, 1, grade(111.0)).unwrap_err();
    assert_eq!(e.kind, "stale", "{e:?}");
    assert!(e.message.contains("open the folder again"), "{e:?}");
    assert_eq!(std::fs::read_to_string(&one).unwrap(), foreign_one);
}

#[test]
fn an_unreadable_sidecar_is_read_only_and_never_rewritten() {
    let dir = Scratch::new("unreadable");
    write_roll(&dir, 2);
    std::fs::write(dir.path("f1.negg.json"), b"{ not json").unwrap();
    let (st, _rec) = state();
    commands::open_roll(&st, dir.dir()).unwrap();
    commands::start_analysis(&st).unwrap();
    wait_idle(&st);
    assert_eq!(
        commands::commit_frame(&st, 1, grade(120.0))
            .unwrap_err()
            .kind,
        "read_only"
    );
    commands::commit_roll(&st, {
        let mut r = commands::roll_view(&st).unwrap().view.roll;
        r.shared.print.grade = 118.0;
        r
    })
    .unwrap();
    wait_idle(&st);
    commands::flush(&st).unwrap();
    assert_eq!(
        std::fs::read(dir.path("f1.negg.json")).unwrap(),
        b"{ not json"
    );
}

#[test]
fn export_sends_progress_and_refuses_after_an_external_change() {
    let dir = Scratch::new("export");
    let out = Scratch::new("export-out");
    write_roll(&dir, 2);
    let (st, rec) = state();
    commands::open_roll(&st, dir.dir()).unwrap();
    commands::start_analysis(&st).unwrap();
    wait_idle(&st);
    commands::commit_frame(&st, 1, grade(122.0)).unwrap();
    rec.take();
    let req = ExportRequest {
        scope: ExportScope::Frames { frames: vec![0, 1] },
        format: ExportFormat::Jpeg,
        out_dir: Some(out.dir()),
        web: negg_session::WebOutput::None,
    };
    let report = commands::export(&st, req.clone()).unwrap();
    assert_eq!((report.converted, report.failed), (2, 0));
    assert!(out.path("f0.jpg").exists() || report.outputs.len() == 2);
    let progress: Vec<(u32, u32, bool)> = rec
        .take()
        .into_iter()
        .filter_map(|e| match e {
            Event::Export(x) => Some((x.done, x.total, x.ok)),
            _ => None,
        })
        .collect();
    assert_eq!(progress, vec![(1, 2, true), (2, 2, true)]);
    // The export flushed the edit first.
    assert_eq!(
        negg_session::read_overrides(&dir.path("f1.tif")).unwrap(),
        grade(122.0)
    );

    // Another program changes a sidecar: the export stops, nothing is
    // overwritten.
    let sidecar = dir.path("f1.negg.json");
    let foreign = std::fs::read_to_string(&sidecar)
        .unwrap()
        .replace("122.0", "101.0");
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(&sidecar, &foreign).unwrap();
    let e = commands::export(&st, req).unwrap_err();
    assert_eq!(e.kind, "stale", "{e:?}");
    assert_eq!(std::fs::read_to_string(&sidecar).unwrap(), foreign);
}

#[test]
fn opening_another_roll_with_unsaved_edits_warns_once() {
    let a = Scratch::new("first");
    let b = Scratch::new("second");
    write_roll(&a, 2);
    write_roll(&b, 1);
    let (st, _rec) = state();
    commands::open_roll(&st, a.dir()).unwrap();
    commands::start_analysis(&st).unwrap();
    wait_idle(&st);
    commands::commit_frame(&st, 0, grade(120.0)).unwrap();
    commands::flush(&st).unwrap();
    let sidecar = a.path("f0.negg.json");
    commands::commit_frame(&st, 0, grade(126.0)).unwrap();
    let foreign = std::fs::read_to_string(&sidecar)
        .unwrap()
        .replace("120.0", "98.0");
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(&sidecar, &foreign).unwrap();

    let e: CommandError = commands::open_roll(&st, b.dir()).unwrap_err();
    assert_eq!(e.kind, "unsaved", "{e:?}");
    // The first roll is still open.
    assert_eq!(commands::roll_view(&st).unwrap().dir, a.dir());
    // Asked again, the other roll opens; the foreign sidecar is untouched.
    let v = commands::open_roll(&st, b.dir()).unwrap();
    assert_eq!(v.dir, b.dir());
    assert_eq!(std::fs::read_to_string(&sidecar).unwrap(), foreign);
    assert!(
        v.awaiting_analysis,
        "a roll opened for the first time waits"
    );
    commands::start_analysis(&st).unwrap();
    wait_idle(&st);
}

#[test]
fn a_roll_edit_marks_every_thumbnail_stale() {
    let dir = Scratch::new("roll-edit");
    write_roll(&dir, 3);
    let (st, rec) = state();
    commands::open_roll(&st, dir.dir()).unwrap();
    commands::start_analysis(&st).unwrap();
    wait_idle(&st);
    rec.take();
    let mut roll = commands::roll_view(&st).unwrap().view.roll;
    roll.pool_film_base = false;
    let v = commands::commit_roll(&st, roll).unwrap();
    assert!(v.pool_pending);
    wait_idle(&st);
    let events = rec.take();
    assert!(events.iter().any(
        |e| matches!(e, Event::Thumbs(t) if t.frames == vec![0, 1, 2] && t.generation >= v.generation)
    ));
    assert!(finished(&events));
    assert_eq!(
        commands::render_tile(&st, 9, 0, 0, 0).unwrap_err().kind,
        "no_frame"
    );
}

/// A roll with a newer commit for frame 0 that cannot be written: its
/// sidecar changed by another program after the commit.
fn unsaved(st: &AppState, dir: &Scratch) -> (PathBuf, String) {
    write_roll(dir, 1);
    commands::open_roll(st, dir.dir()).unwrap();
    commands::start_analysis(st).unwrap();
    wait_idle(st);
    commands::commit_frame(st, 0, grade(120.0)).unwrap();
    commands::flush(st).unwrap();
    let sidecar = dir.path("f0.negg.json");
    commands::commit_frame(st, 0, grade(127.0)).unwrap();
    let foreign = std::fs::read_to_string(&sidecar)
        .unwrap()
        .replace("120.0", "97.0");
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(&sidecar, &foreign).unwrap();
    (sidecar, foreign)
}

#[test]
fn closing_with_unsaved_edits_warns_once_per_edit() {
    let (st, _rec) = state();
    st.prepare_close().unwrap();
    let dir = Scratch::new("close");
    let (sidecar, foreign) = unsaved(&st, &dir);
    assert_eq!(st.prepare_close().unwrap_err().kind, "unsaved");
    st.prepare_close().unwrap();
    assert_eq!(std::fs::read_to_string(&sidecar).unwrap(), foreign);
}

#[test]
fn a_new_edit_rearms_the_close_warning() {
    // A sidecar path that is a directory cannot be replaced, so every
    // flush of frame 0 fails.
    let (st, _rec) = state();
    let dir2 = Scratch::new("rearm");
    write_roll(&dir2, 1);
    commands::open_roll(&st, dir2.dir()).unwrap();
    commands::start_analysis(&st).unwrap();
    wait_idle(&st);
    std::fs::create_dir(dir2.path("f0.negg.json")).unwrap();
    commands::commit_frame(&st, 0, grade(121.0)).unwrap();
    assert_eq!(st.prepare_close().unwrap_err().kind, "unsaved");
    st.prepare_close().unwrap();
    commands::commit_frame(&st, 0, grade(122.0)).unwrap();
    assert_eq!(
        st.prepare_close().unwrap_err().kind,
        "unsaved",
        "a new edit must warn again"
    );
}

#[test]
fn closing_during_an_export_is_refused_without_waiting() {
    let (st, _rec) = state();
    let e = st.exporting(|| st.prepare_close()).unwrap_err();
    assert_eq!(e.kind, "exporting");
    st.prepare_close().unwrap();
    assert!(st.begin_close());
    assert!(!st.begin_close(), "one close at a time");
    st.end_close(false);
    assert!(!st.close_approved());
    assert!(st.begin_close());
    st.end_close(true);
    assert!(st.close_approved());
}

#[test]
fn the_exit_flush_writes_a_recent_commit() {
    let (st, _rec) = state();
    assert!(!st.flush_on_exit(Duration::from_millis(50)));
    let dir = Scratch::new("exit");
    write_roll(&dir, 1);
    commands::open_roll(&st, dir.dir()).unwrap();
    commands::start_analysis(&st).unwrap();
    wait_idle(&st);
    commands::commit_frame(&st, 0, grade(124.0)).unwrap();
    // Within the debounce: nothing written yet.
    assert!(!dir.path("f0.negg.json").exists());
    assert!(st.flush_on_exit(Duration::from_secs(5)));
    assert_eq!(
        negg_session::read_overrides(&dir.path("f0.tif")).unwrap(),
        grade(124.0)
    );
}

const AREA: CropRect = CropRect {
    x: 0.05,
    y: 0.05,
    width: 0.9,
    height: 0.9,
};

#[test]
fn before_analyse_the_negatives_show_and_the_area_is_set_without_analysing() {
    let dir = Scratch::new("pre");
    write_roll(&dir, 3);
    let before = snapshot(&dir.0);
    let (st, rec) = state();
    let view = commands::open_roll(&st, dir.dir()).unwrap();
    assert!(view.awaiting_analysis);
    assert!(!view.analysed);

    // The negatives, with no analysis.
    let jpeg = commands::negative_thumbnail(&st, 0).unwrap();
    assert_eq!(&jpeg[..2], &[0xff, 0xd8]);
    let nv = commands::negative_view(&st, 1).unwrap();
    assert_eq!((nv.index, nv.width, nv.height), (1, 96, 64));
    let tile = commands::negative_tile(&st, 1, 0, 0, 0).unwrap();
    let (header, _) = parse_tile(&tile).unwrap();
    assert_eq!((header.frame, header.width, header.height), (1, 96, 64));
    // The positive waits for pass 1, also for frame 1, open for its
    // negative (C-15).
    assert_eq!(commands::frame_view(&st, 1).unwrap_err().kind, "busy");
    assert_eq!(commands::frame_view(&st, 2).unwrap_err().kind, "busy");
    assert_eq!(
        snapshot(&dir.0),
        before,
        "viewing the negatives wrote something"
    );

    // The roll's area: recorded, and no edit or flush starts the analysis.
    let v = commands::set_roll_analysis_area(&st, Some(AREA)).unwrap();
    assert_eq!(v.roll.shared.geometry.analysis_area, Some(AREA));
    assert!(v.awaiting_analysis);
    let invalid = CropRect {
        x: 0.5,
        width: 0.9,
        ..AREA
    };
    assert_eq!(
        commands::set_roll_analysis_area(&st, Some(invalid))
            .unwrap_err()
            .kind,
        "invalid"
    );
    std::thread::sleep(FLUSH_DELAY * 3);
    commands::flush(&st).unwrap();
    assert!(!st.is_analysing());
    let v = commands::roll_view(&st).unwrap();
    assert!(v.awaiting_analysis && !v.analysed);
    assert!(v.frames.iter().all(|f| f.status == FrameStatus::Pending));
    assert!(
        !rec.take().iter().any(|e| matches!(e, Event::Analysis(_))),
        "analysed before Analyse"
    );
    // Only the committed roll settings were written; no frame sidecar.
    let after = snapshot(&dir.0);
    let names: Vec<&str> = after.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        vec!["f0.tif", "f1.tif", "f2.tif", "roll.negg.json"],
        "{names:?}"
    );

    // Analyse.
    let v = commands::start_analysis(&st).unwrap();
    assert!(!v.awaiting_analysis);
    wait_idle(&st);
    assert!(finished(&rec.take()));
    let v = commands::roll_view(&st).unwrap();
    assert!(v.frames.iter().all(|f| f.status == FrameStatus::Ready));
    assert_eq!(v.roll.shared.geometry.analysis_area, Some(AREA));

    // A frame's own area, then the roll's area changed after analysis:
    // both re-pool in the background.
    let o = FrameOverrides {
        geometry: Some(Geometry {
            analysis_area: Some(CropRect {
                x: 0.1,
                y: 0.1,
                width: 0.8,
                height: 0.8,
            }),
            ..v.roll.shared.geometry
        }),
        ..FrameOverrides::default()
    };
    let fv = commands::commit_frame(&st, 0, o).unwrap();
    assert!(fv.pool_pending);
    wait_idle(&st);
    let v = commands::set_roll_analysis_area(&st, None).unwrap();
    assert!(v.pool_pending);
    wait_idle(&st);
    assert!(finished(&rec.take()));
    let fv = commands::frame_view(&st, 0).unwrap();
    assert!(
        fv.settings.geometry.analysis_area.is_some(),
        "the frame keeps its own"
    );
    let fv = commands::frame_view(&st, 1).unwrap();
    assert_eq!(fv.settings.geometry.analysis_area, None);
}

#[test]
fn a_roll_opened_before_is_analysed_at_once() {
    let dir = Scratch::new("again");
    write_roll(&dir, 2);
    let (st, _rec) = state();
    commands::open_roll(&st, dir.dir()).unwrap();
    commands::set_roll_analysis_area(&st, Some(AREA)).unwrap();
    commands::flush(&st).unwrap();
    assert!(dir.path("roll.negg.json").exists());

    let (st2, rec2) = state();
    let v = commands::open_roll(&st2, dir.dir()).unwrap();
    assert!(!v.awaiting_analysis);
    assert_eq!(v.roll.shared.geometry.analysis_area, Some(AREA));
    wait_idle(&st2);
    assert!(finished(&rec2.take()));
}

#[test]
fn the_close_handshake_waits_for_the_webview_and_not_for_ever() {
    let (st, rec) = state();
    // No answer: the close goes ahead after the wait.
    let t = Instant::now();
    assert!(!st.await_frontend(Duration::from_millis(50)));
    assert!(t.elapsed() >= Duration::from_millis(50));
    assert!(rec.take().contains(&Event::Closing));
    // The webview answers once its edits are sent.
    let waiter = {
        let st = st.clone();
        std::thread::spawn(move || st.await_frontend(Duration::from_secs(20)))
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    while !rec.take().contains(&Event::Closing) {
        assert!(Instant::now() < deadline, "no negg://closing");
        std::thread::sleep(Duration::from_millis(5));
    }
    commands::close_ready(&st);
    assert!(waiter.join().unwrap());
    assert_eq!(Event::Closing.name(), "negg://closing");
}

// The `_in` command variants take the presets folder, so no test here
// touches `NEGG_PRESETS_DIR` (that is covered by presets_env.rs).
#[test]
fn apply_roll_preset_command_records_one_history_entry() {
    let dir = Scratch::new("rollpreset");
    write_roll(&dir, 3);
    let presets = Scratch::new("rollpreset-dir");
    let (st, rec) = state();
    commands::open_roll(&st, dir.dir()).unwrap();
    commands::start_analysis(&st).unwrap();
    wait_idle(&st);

    // Frame 0 gets a look of its own.
    commands::commit_frame(&st, 0, grade(130.0)).unwrap();
    assert_eq!(commands::frames_with_own_look(&st).unwrap(), vec![0]);
    let before = commands::roll_view(&st).unwrap().history.len;
    rec.take();

    let view = commands::apply_roll_preset_in(
        &st,
        Some(&presets.0),
        "Rich",
        negg_session::RollPresetScope::AllFrames,
    )
    .unwrap();
    assert_eq!(view.history.len, before + 1);
    assert!(view.history.can_undo);
    assert!(!view.awaiting_analysis);
    assert!(commands::frames_with_own_look(&st).unwrap().is_empty());
    let events = rec.take();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::Thumbs(t) if t.frames == vec![0, 1, 2])),
        "{events:?}"
    );

    // One undo takes the whole apply back.
    commands::undo(&st).unwrap();
    assert_eq!(commands::frames_with_own_look(&st).unwrap(), vec![0]);

    // An unknown name is an error naming the choices.
    let e = commands::apply_roll_preset_in(
        &st,
        Some(&presets.0),
        "Nope",
        negg_session::RollPresetScope::AllFrames,
    )
    .unwrap_err();
    assert!(e.message.contains("no preset named"), "{}", e.message);
}

#[test]
fn frame_preset_save_and_thumbnail_commands() {
    let dir = Scratch::new("framepreset");
    write_roll(&dir, 2);
    let presets = Scratch::new("framepreset-dir");
    let (st, _) = state();
    commands::open_roll(&st, dir.dir()).unwrap();
    commands::start_analysis(&st).unwrap();
    wait_idle(&st);

    let list = commands::list_presets_in(Some(&presets.0));
    assert!(list.presets.iter().any(|p| p.name == "Rich"));
    let rich = list.presets.iter().find(|p| p.name == "Rich").unwrap();
    let fv = commands::apply_frame_preset_in(&st, Some(&presets.0), 1, "Rich").unwrap();
    assert_eq!(fv.overrides.print.preset.as_deref(), Some("Rich"));
    assert_eq!(fv.overrides.print.grade, Some(rich.look.grade));

    let jpeg = commands::preset_thumbnail_in(&st, Some(&presets.0), 0, "Rich").unwrap();
    assert_eq!(&jpeg[..2], &[0xff, 0xd8]);

    let saved = commands::save_preset_in(&presets.0, "Mine", rich.look).unwrap();
    assert!(saved.presets.iter().any(|p| p.name == "Mine"));
    assert!(saved.warnings.is_empty());
    let e = commands::save_preset_in(&presets.0, "Rich", rich.look).unwrap_err();
    assert!(e.message.contains("built-in"), "{}", e.message);
}
