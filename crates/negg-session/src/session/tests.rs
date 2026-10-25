// SPDX-License-Identifier: Apache-2.0

//! Session tests on synthetic rolls written to a scratch directory.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use negg_types::settings::ChannelEstimate;
use negg_types::{CropRect, PoolRole};

use super::*;
use crate::wire::{SCOPES_LEN, TILE_HEADER_LEN, parse_tile};

/// A scratch directory removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "negg-session-s-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A synthetic negative as an untagged 16-bit TIFF: a clear border of
/// density `dmin` around a ramp of density up to `dmin + 1.6` with a
/// checker texture, stored as gamma 2.2 encoded transmittance.
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

/// A roll of `n` frames of `w` by `h` with slightly different bases.
fn write_roll(dir: &Scratch, n: usize, w: usize, h: usize) {
    for i in 0..n {
        let d = 0.01 * i as f32;
        write_negative(
            &dir.path(&format!("f{i}.tif")),
            w,
            h,
            [0.25 + d, 0.55 + d, 0.75 + d],
        );
    }
}

/// Opens the roll and runs pass 1 to the end.
fn analysed(dir: &Scratch) -> Session {
    let mut s = Session::open_roll(&dir.0).unwrap();
    s.analyse_all().unwrap();
    s
}

/// Every file in `dir` with its length and modification time.
fn listing(dir: &Path) -> Vec<(String, u64, SystemTime)> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            let m = e.metadata().unwrap();
            (
                e.file_name().to_string_lossy().into_owned(),
                m.len(),
                m.modified().unwrap(),
            )
        })
        .collect();
    v.sort();
    v
}

fn grade(g: f32) -> FrameOverrides {
    let mut o = FrameOverrides::default();
    o.print.grade = Some(g);
    o
}

#[test]
fn open_and_analysis_write_nothing() {
    let dir = Scratch::new("open");
    write_roll(&dir, 2, 96, 64);
    std::fs::write(dir.path("f1.negg.json"), b"{ not json").unwrap();
    let before = listing(&dir.0);
    let mut s = analysed(&dir);
    s.frame_view(0).unwrap();
    s.render_tile(0, 0, 0, 0).unwrap();
    s.thumbnail_jpeg(0).unwrap();
    s.scopes(0).unwrap();
    assert!(!s.is_dirty());
    assert_eq!(listing(&dir.0), before);
}

#[test]
fn analysis_runs_to_done_on_a_four_frame_roll() {
    let dir = Scratch::new("analyse");
    write_roll(&dir, 4, 96, 64);
    let mut s = Session::open_roll(&dir.0).unwrap();
    let view = s.roll_view();
    assert!(!view.analysed);
    assert_eq!(view.frames.len(), 4);
    assert!(view.frames.iter().all(|f| f.status == FrameStatus::Pending));
    assert!(matches!(
        s.commit_frame(0, grade(120.0)),
        Err(SessionError::Busy)
    ));
    let mut steps = Vec::new();
    loop {
        let step = s.analyse_next().unwrap();
        steps.push(step);
        if step == AnalyseStep::Done {
            break;
        }
    }
    let expected: Vec<AnalyseStep> = (0..4)
        .map(|frame| AnalyseStep::Analysed { frame, ok: true })
        .chain([AnalyseStep::Pooled, AnalyseStep::Done])
        .collect();
    assert_eq!(steps, expected);
    assert_eq!(s.analyse_next().unwrap(), AnalyseStep::Done);
    let view = s.roll_view();
    assert!(view.analysed);
    assert!(view.frames.iter().all(|f| f.status == FrameStatus::Ready));
    let pool = s.pool.as_ref().unwrap();
    assert_eq!(pool.frames.len(), 4);
    assert!(pool.frames.iter().all(|f| f.dmin_role == PoolRole::Inlier));
    let fv = s.frame_view(2).unwrap();
    assert_eq!((fv.width, fv.height, fv.levels), (96, 64, 1));
    assert!(fv.estimates.is_some() && fv.pool.is_some());
}

#[test]
fn level_0_tiles_equal_the_render_and_headers_are_right() {
    let dir = Scratch::new("tiles");
    write_roll(&dir, 2, 600, 400);
    let mut s = analysed(&dir);
    let view = s.frame_view(1).unwrap();
    assert_eq!((view.width, view.height, view.levels), (600, 400, 2));

    // Reference: the frame rendered whole as pass 2 of `negg batch` does.
    let prepared = prepare_frame(&dir.path("f1.tif"), None).unwrap();
    let pooled = s.pool.as_ref().unwrap().pooled(&s.roll);
    let ctx = RenderContext {
        pooled: Some(&pooled),
        ..RenderContext::default()
    };
    let render = negg_render::render_frame_with(
        &prepared.linear,
        &view.settings,
        &ctx,
        prepared.camera_to_working,
    )
    .unwrap();
    assert_eq!(Some(render.estimates), view.estimates);
    let srgb = negg_cms::working_to_output(OutputSpace::Srgb, Intent::Perceptual).unwrap();
    let whole = srgb.apply_u8(&render.image);

    let mut assembled = vec![0u8; 600 * 400 * 3];
    for (tx, width) in [(0u32, 512usize), (1, 88)] {
        let bytes = s.render_tile(1, 0, tx, 0).unwrap();
        let (header, pixels) = parse_tile(&bytes).unwrap();
        assert_eq!(
            header,
            crate::wire::TileHeader {
                width: width as u32,
                height: 400,
                tile_x: tx,
                tile_y: 0,
                level: 0,
                frame: 1,
                generation: view.generation,
            }
        );
        for y in 0..400 {
            for x in 0..width {
                let p = &pixels[(y * width + x) * 4..][..4];
                assert_eq!(p[3], 255);
                let at = (y * 600 + tx as usize * 512 + x) * 3;
                assembled[at..at + 3].copy_from_slice(&p[..3]);
            }
        }
    }
    assert!(assembled == whole, "level 0 tiles differ from the render");

    // Level 1 is one 300 x 200 tile; anything else is out of range.
    let (header, _) = parse_tile(&s.render_tile(1, 1, 0, 0).unwrap()).unwrap();
    assert_eq!((header.width, header.height), (300, 200));
    assert!(matches!(
        s.render_tile(1, 0, 2, 0),
        Err(SessionError::Invalid(_))
    ));
    assert!(matches!(
        s.render_tile(1, 2, 0, 0),
        Err(SessionError::Invalid(_))
    ));
    assert!(matches!(
        s.render_tile(9, 0, 0, 0),
        Err(SessionError::NoFrame(9))
    ));
}

#[test]
fn scopes_and_thumbnails() {
    let dir = Scratch::new("scopes");
    write_roll(&dir, 1, 96, 64);
    let mut s = analysed(&dir);
    let bytes = s.scopes(0).unwrap();
    assert_eq!(bytes.len(), 528_416);
    assert_eq!(bytes.len(), SCOPES_LEN);
    assert_eq!(&bytes[..8], b"NEGGSCOP");
    let at = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
    assert_eq!((at(12), at(16), at(20)), (0, s.generation, 32));
    assert_eq!((at(24), at(28)), (96, 64));
    let pixels: u32 = (0..256).map(|b| at(32 + 3 * 1024 + b * 4)).sum();
    assert_eq!(pixels, 96 * 64, "luma histogram counts every pixel");

    // Scopes describe the exported framing: rotate 90.
    let o = FrameOverrides {
        geometry: Some(Geometry {
            rotate_degrees: 90,
            ..Geometry::default()
        }),
        ..FrameOverrides::default()
    };
    s.commit_frame(0, o).unwrap();
    let bytes = s.scopes(0).unwrap();
    let at = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
    assert_eq!((at(24), at(28)), (64, 96));

    let jpeg = s.thumbnail_jpeg(0).unwrap();
    assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
    assert_eq!(s.thumbnail_jpeg(0).unwrap(), jpeg, "cached");
}

#[test]
fn commit_then_flush_writes_sidecars_read_overrides_returns() {
    let dir = Scratch::new("flush");
    write_roll(&dir, 2, 96, 64);
    let mut s = analysed(&dir);
    let mut o = grade(140.0);
    o.print.exposure = Some(0.2);
    let view = s.commit_frame(1, o.clone()).unwrap();
    assert!(s.is_dirty());
    assert_eq!(view.overrides, o);
    assert!(
        !dir.path("f1.negg.json").exists(),
        "commit alone writes nothing"
    );
    s.flush().unwrap();
    assert!(!s.is_dirty());
    assert_eq!(read_overrides(&dir.path("f1.tif")).unwrap(), o);
    // The frame not edited is written as `negg batch` writes it, since
    // the roll is now written (Ruling 139).
    let zero = negg_io::read_sidecar(&dir.path("f0.negg.json")).unwrap();
    assert_eq!(zero.overrides, FrameOverrides::default());
    assert_eq!(zero.estimates, s.frame_view(0).unwrap().estimates);
    let sidecar = negg_io::read_sidecar(&dir.path("f1.negg.json")).unwrap();
    assert_eq!(sidecar.estimates, view.estimates);
    assert_eq!(sidecar.settings, view.settings);
    assert_eq!(sidecar.roll.as_deref(), Some(ROLL_SIDECAR_NAME));
    let roll = negg_io::read_roll_sidecar(&dir.path(ROLL_SIDECAR_NAME)).unwrap();
    assert_eq!(roll.pool.as_ref(), s.pool.as_ref());
    assert_eq!(roll.frames, ["f0.tif", "f1.tif"]);

    // A second flush with nothing new writes nothing.
    let before = listing(&dir.0);
    s.flush().unwrap();
    assert_eq!(listing(&dir.0), before);

    // A reopened session reads the overrides back.
    let s2 = Session::open_roll(&dir.0).unwrap();
    assert_eq!(s2.frames[1].overrides, o);
}

#[test]
fn flush_after_an_external_change_refuses_and_turns_read_only() {
    let dir = Scratch::new("stale");
    write_roll(&dir, 2, 96, 64);
    let mut s = analysed(&dir);
    s.commit_frame(0, grade(130.0)).unwrap();
    s.flush().unwrap();
    let path = dir.path("f0.negg.json");
    let touch = |when: SystemTime| {
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(when)
            .unwrap();
    };
    // The same bytes written again (as `negg batch` would) are not a
    // change.
    touch(SystemTime::now() - Duration::from_secs(7200));
    s.commit_frame(0, grade(140.0)).unwrap();
    s.flush().unwrap();
    // Someone else edits the sidecar after a commit and before the flush:
    // the flush refuses it.
    s.commit_frame(0, grade(150.0)).unwrap();
    s.commit_frame(1, grade(90.0)).unwrap();
    let external = b"{\"schema_version\": 3}\n";
    std::fs::write(&path, external).unwrap();
    touch(SystemTime::now() - Duration::from_secs(3600));
    let err = s.flush().unwrap_err();
    assert!(
        matches!(&err, SessionError::Stale(p) if *p == path),
        "{err}"
    );
    assert!(err.to_string().contains("open the folder again"), "{err}");
    assert_eq!(std::fs::read(&path).unwrap(), external, "never overwritten");
    assert!(
        dir.path("f1.negg.json").is_file(),
        "the other frame is written"
    );
    let view = s.roll_view();
    assert_eq!(view.frames[0].status, FrameStatus::ReadOnly);
    let err = s.commit_frame(0, grade(100.0)).unwrap_err();
    assert!(matches!(err, SessionError::ReadOnly(0)));
    assert!(
        err.to_string().starts_with("frame 1 is read-only"),
        "the 1-based number the app shows: {err}"
    );
    assert!(err.to_string().contains("open the folder again"), "{err}");
    // It shows what is on disk, and its edits left the history, so undo
    // is not blocked by it.
    assert_eq!(s.frames[0].overrides, FrameOverrides::default());
    assert_eq!(s.history_view().len, 1);
    s.undo().unwrap();
    assert_eq!(s.frames[1].overrides, FrameOverrides::default());
    s.flush().unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), external);
}

#[test]
fn an_unreadable_sidecar_is_read_only_and_never_rewritten() {
    let dir = Scratch::new("unreadable");
    write_roll(&dir, 3, 96, 64);
    let path = dir.path("f1.negg.json");
    let junk = b"{ this is not json";
    std::fs::write(&path, junk).unwrap();
    let mut s = analysed(&dir);
    let view = s.roll_view();
    assert_eq!(view.frames[1].status, FrameStatus::ReadOnly);
    assert!(view.frames[1].message.is_some());
    assert!(s.frame_view(1).unwrap().read_only);
    assert!(matches!(
        s.commit_frame(1, grade(120.0)),
        Err(SessionError::ReadOnly(1))
    ));
    assert!(matches!(
        s.preview_frame(1, grade(120.0)),
        Err(SessionError::ReadOnly(1))
    ));
    assert!(matches!(
        s.lock_frame(1, LockWhich::default()),
        Err(SessionError::ReadOnly(1))
    ));
    // Left out of the pool, as `negg batch` leaves it out.
    let pool = s.pool.as_ref().unwrap();
    assert_eq!(
        pool.frames
            .iter()
            .map(|f| f.source.as_str())
            .collect::<Vec<_>>(),
        ["f0.tif", "f2.tif"]
    );
    let mut roll = s.roll.clone();
    roll.shared.print.grade = 120.0;
    s.commit_roll(roll).unwrap();
    s.commit_frame(0, grade(130.0)).unwrap();
    s.flush().unwrap();
    let mut events = Vec::new();
    let report = s
        .export(
            &ExportRequest {
                scope: ExportScope::Roll,
                format: ExportFormat::Jpeg,
                out_dir: Some(dir.path("out").to_string_lossy().into_owned()),
            },
            &mut |e| events.push(e),
        )
        .unwrap();
    assert_eq!((report.converted, report.failed), (2, 1));
    assert!(report.errors[0].contains("f1.tif"), "{:?}", report.errors);
    assert_eq!(std::fs::read(&path).unwrap(), junk);
}

#[test]
fn history_cap_no_op_commit_and_redo_tail() {
    let dir = Scratch::new("history");
    write_roll(&dir, 1, 48, 32);
    let mut s = analysed(&dir);
    let g0 = s.generation;
    s.commit_frame(0, FrameOverrides::default()).unwrap();
    assert_eq!(s.history_view().len, 0, "a no-op commit is not recorded");
    assert!(!s.is_dirty());
    assert!(s.generation > g0);

    s.commit_frame(0, grade(100.0)).unwrap();
    s.commit_frame(0, grade(110.0)).unwrap();
    s.commit_frame(0, grade(120.0)).unwrap();
    s.undo().unwrap();
    let h = s.undo().unwrap();
    assert_eq!(
        (h.len, h.cursor, h.can_undo, h.can_redo),
        (3, 1, true, true)
    );
    assert_eq!(s.frames[0].overrides, grade(100.0));
    let h = s.redo().unwrap();
    assert_eq!(h.cursor, 2);
    assert_eq!(s.frames[0].overrides, grade(110.0));
    // A commit after an undo drops the redo tail.
    s.undo().unwrap();
    let v = s.commit_frame(0, grade(70.0)).unwrap();
    assert_eq!((v.history.len, v.history.cursor), (2, 2));
    assert!(!v.history.can_redo);
    assert_eq!(s.redo().unwrap().cursor, 2, "nothing to redo");

    // The cap drops the oldest.
    for k in 0..HISTORY_LIMIT + 5 {
        s.commit_frame(0, grade(60.0 + (k % 100) as f32 + 0.5))
            .unwrap();
    }
    let h = s.history_view();
    assert_eq!(
        (h.len as usize, h.cursor as usize),
        (HISTORY_LIMIT, HISTORY_LIMIT)
    );
    for _ in 0..HISTORY_LIMIT {
        s.undo().unwrap();
    }
    assert!(!s.history_view().can_undo);
    assert_eq!(s.undo().unwrap().cursor, 0);
    // The oldest edits kept start after the dropped ones.
    assert_ne!(s.frames[0].overrides, grade(70.0));
}

#[test]
fn locking_dmin_pools_again_and_a_grade_change_does_not() {
    let dir = Scratch::new("repool");
    write_roll(&dir, 3, 96, 64);
    let mut s = analysed(&dir);
    let before = s.frame_view(0).unwrap();
    s.commit_frame(0, grade(130.0)).unwrap();
    assert!(!s.pool_pending, "a grade change leaves the pool");
    let which = LockWhich {
        dmin: true,
        ..LockWhich::default()
    };
    let view = s.lock_frame(0, which).unwrap();
    assert!(s.pool_pending, "locking Dmin pools again");
    assert!(s.roll_view().pool_pending);
    assert_eq!(
        view.overrides.film.film_base,
        Some(ChannelEstimate::Fixed {
            value: before.estimates.unwrap().dmin
        })
    );
    assert_eq!(view.overrides.print.grade, Some(130.0));
    // The host drives the pending re-pool.
    assert_eq!(s.analyse_next().unwrap(), AnalyseStep::Pooled);
    assert_eq!(s.analyse_next().unwrap(), AnalyseStep::Done);
    assert!(!s.roll_view().pool_pending);
    let view = s.frame_view(0).unwrap();
    assert_eq!(view.pool.unwrap().dmin_role, PoolRole::Excluded);
    // Undo is a commit too: the lock goes and the pool is pending again.
    s.undo().unwrap();
    assert!(s.pool_pending);
    assert_eq!(s.frames[0].overrides, grade(130.0));
    s.analyse_all().unwrap();
    // Rotation and flip leave the pool; a crop does not.
    let mut o = grade(130.0);
    o.geometry = Some(Geometry {
        rotate_degrees: 270,
        flip_horizontal: true,
        ..Geometry::default()
    });
    s.commit_frame(0, o.clone()).unwrap();
    assert!(!s.pool_pending);
    o.geometry = Some(Geometry {
        crop: Some(CropRect {
            x: 0.1,
            y: 0.1,
            width: 0.8,
            height: 0.8,
        }),
        ..Geometry::default()
    });
    s.commit_frame(0, o).unwrap();
    assert!(s.pool_pending);
    s.analyse_all().unwrap();
    // A pool flag pools again.
    let mut roll = s.roll.clone();
    roll.pool_colour = false;
    s.commit_roll(roll).unwrap();
    assert!(s.pool_pending);
    // A flush runs a pending re-pool first.
    s.flush().unwrap();
    assert!(!s.pool_pending);
}

#[test]
fn a_re_pool_that_changes_nothing_keeps_the_caches() {
    let dir = Scratch::new("keep");
    write_roll(&dir, 3, 96, 64);
    let mut s = analysed(&dir);
    s.thumbnail_jpeg(1).unwrap();
    let key = s.frames[1].thumb.as_ref().unwrap().0;
    let generation = s.generation;
    // Toggling a pool flag off and on again pools twice and ends where it
    // began.
    let roll = s.roll.clone();
    let mut off = roll.clone();
    off.pool_colour = false;
    s.commit_roll(off).unwrap();
    s.commit_roll(roll).unwrap();
    assert!(s.pool_pending);
    let g = s.generation;
    s.analyse_all().unwrap();
    assert_eq!(
        s.generation, g,
        "an unchanged pool does not move the generation"
    );
    assert!(s.generation > generation);
    assert_eq!(s.frames[1].thumb.as_ref().unwrap().0, key);
    let thumb = s.thumbnail_jpeg(1).unwrap();
    assert_eq!(s.frames[1].thumb.as_ref().unwrap().0, key);
    assert_eq!(thumb, s.frames[1].thumb.as_ref().unwrap().1);
}

#[test]
fn a_refused_commit_keeps_the_preview() {
    let dir = Scratch::new("keep-preview");
    write_roll(&dir, 1, 48, 32);
    let mut s = analysed(&dir);
    s.preview_frame(0, grade(150.0)).unwrap();
    let mut tiny = grade(150.0);
    tiny.geometry = Some(Geometry {
        crop: Some(CropRect {
            x: 0.0,
            y: 0.0,
            width: 0.05,
            height: 0.05,
        }),
        ..Geometry::default()
    });
    assert!(s.commit_frame(0, tiny).is_err());
    assert_eq!(s.frames[0].preview, Some(grade(150.0)));
}

#[test]
fn export_refuses_external_edits_and_never_overwrites_them() {
    let dir = Scratch::new("two-sessions");
    write_roll(&dir, 2, 96, 64);
    let mut a = analysed(&dir);
    let mut b = analysed(&dir);
    b.commit_frame(0, grade(50.0)).unwrap();
    let mut roll = b.roll.clone();
    roll.shared.print.grade = 90.0;
    b.commit_roll(roll).unwrap();
    b.flush().unwrap();
    let frame0 = std::fs::read(dir.path("f0.negg.json")).unwrap();
    let roll_file = std::fs::read(dir.path(ROLL_SIDECAR_NAME)).unwrap();

    let req = ExportRequest {
        scope: ExportScope::Roll,
        format: ExportFormat::Jpeg,
        out_dir: Some(dir.path("out").to_string_lossy().into_owned()),
    };
    let err = a.export(&req, &mut |_| {}).unwrap_err();
    assert!(matches!(err, SessionError::Stale(_)), "{err}");
    assert!(!dir.path("out").exists(), "nothing is exported");
    // A shows what is on disk for the frame, and cannot edit it.
    assert_eq!(a.frames[0].overrides, grade(50.0));
    assert_eq!(a.roll_view().frames[0].status, FrameStatus::ReadOnly);
    assert!(matches!(
        a.commit_frame(0, grade(130.0)),
        Err(SessionError::ReadOnly(0))
    ));
    // A's own roll edit is refused on flush, never written over B's.
    let mut roll = a.roll.clone();
    roll.shared.print.grade = 70.0;
    a.commit_roll(roll).unwrap();
    assert!(matches!(a.flush(), Err(SessionError::Stale(_))));
    assert!(matches!(
        a.export(&req, &mut |_| {}),
        Err(SessionError::Stale(_))
    ));
    assert_eq!(std::fs::read(dir.path("f0.negg.json")).unwrap(), frame0);
    assert_eq!(
        std::fs::read(dir.path(ROLL_SIDECAR_NAME)).unwrap(),
        roll_file
    );
}

#[test]
fn roll_settings_are_written_while_pass_1_runs_again() {
    let dir = Scratch::new("window");
    write_roll(&dir, 2, 48, 32);
    let mut s = analysed(&dir);
    s.commit_frame(0, grade(120.0)).unwrap();
    let mut roll = s.roll.clone();
    roll.shared.print.grade = 95.0;
    roll.shared.profiles.film = Some(dir.path("missing.film.negg.toml").to_string_lossy().into());
    s.commit_roll(roll.clone()).unwrap();
    assert!(!s.roll_view().analysed);
    // Closing now must not lose the roll settings or the frame edit.
    s.flush().unwrap();
    let disk = negg_io::read_roll_sidecar(&dir.path(ROLL_SIDECAR_NAME)).unwrap();
    assert_eq!(disk.settings, roll);
    assert_eq!(disk.pool, None, "no pool while pass 1 runs again");
    assert_eq!(disk.frames, ["f0.tif", "f1.tif"]);
    let side = negg_io::read_sidecar(&dir.path("f0.negg.json")).unwrap();
    assert_eq!(side.overrides, grade(120.0));
    assert_eq!(side.estimates, None, "not analysed yet");
    assert!(!s.is_dirty());
    // The profile does not load, so every frame fails pass 1; the pool
    // stays empty and the sidecars keep no estimates.
    s.analyse_all().unwrap();
    assert!(!s.is_dirty());
    // Undo restores the profiles; after pass 1 the next flush writes the
    // frame's estimates and the pool.
    s.undo().unwrap();
    s.analyse_all().unwrap();
    s.flush().unwrap();
    let side = negg_io::read_sidecar(&dir.path("f0.negg.json")).unwrap();
    assert!(side.estimates.is_some());
    assert_eq!(side.estimates, s.frame_view(0).unwrap().estimates);
    let disk = negg_io::read_roll_sidecar(&dir.path(ROLL_SIDECAR_NAME)).unwrap();
    assert!(disk.pool.is_some());
    assert_eq!(disk.settings, s.roll);
}

#[test]
fn level_count_follows_the_pyramid() {
    for (w, h) in [
        (6000, 4000),
        (512, 300),
        (513, 2),
        (1025, 3),
        (0, 0),
        (300, 4000),
    ] {
        let want = pyramid(&Image::new(w, h), TILE_SIZE as usize).len() as u32 + 1;
        assert_eq!(level_count(w, h), want, "{w} x {h}");
    }
    assert_eq!(level_count(6000, 4000), 5);
}

#[test]
fn a_crop_too_small_to_analyse_is_refused() {
    let dir = Scratch::new("tiny");
    write_roll(&dir, 1, 48, 32);
    let mut s = analysed(&dir);
    let o = FrameOverrides {
        geometry: Some(Geometry {
            crop: Some(CropRect {
                x: 0.0,
                y: 0.0,
                width: 0.05,
                height: 0.05,
            }),
            ..Geometry::default()
        }),
        ..FrameOverrides::default()
    };
    assert!(matches!(
        s.commit_frame(0, o),
        Err(SessionError::Invalid(_))
    ));
    assert_eq!(s.history_view().len, 0);
    assert!(!s.is_dirty());
}

#[test]
fn previews_draw_without_history_or_sidecar() {
    let dir = Scratch::new("preview");
    write_roll(&dir, 1, 96, 64);
    let mut s = analysed(&dir);
    let committed = s.thumbnail_jpeg(0).unwrap();
    let g = s.preview_frame(0, grade(170.0)).unwrap();
    assert_eq!(g, s.generation);
    let previewed = s.thumbnail_jpeg(0).unwrap();
    assert_ne!(previewed, committed);
    let (header, _) = parse_tile(&s.render_tile(0, 0, 0, 0).unwrap()).unwrap();
    assert_eq!(header.generation, g);
    assert_eq!(s.history_view().len, 0);
    assert!(!s.is_dirty());
    assert_eq!(
        s.frame_view(0).unwrap().overrides,
        FrameOverrides::default()
    );
    // Committing the unchanged value ends the preview.
    s.commit_frame(0, FrameOverrides::default()).unwrap();
    assert_eq!(s.thumbnail_jpeg(0).unwrap(), committed);
}

#[test]
fn new_profiles_start_pass_1_again() {
    let dir = Scratch::new("profiles");
    write_roll(&dir, 2, 48, 32);
    let mut s = analysed(&dir);
    let mut roll = s.roll.clone();
    roll.shared.profiles.film = Some(dir.path("missing.film.negg.toml").to_string_lossy().into());
    let view = s.commit_roll(roll).unwrap();
    assert!(!view.analysed);
    assert!(matches!(
        s.commit_frame(0, grade(90.0)),
        Err(SessionError::Busy)
    ));
    s.analyse_all().unwrap();
    let view = s.roll_view();
    assert!(view.frames.iter().all(|f| f.status == FrameStatus::Failed));
    assert!(
        view.frames[0]
            .message
            .as_deref()
            .unwrap()
            .contains("missing.film")
    );
    // Undo restores the profiles and pass 1 runs again.
    s.undo().unwrap();
    assert!(!s.roll_view().analysed);
    s.analyse_all().unwrap();
    assert!(
        s.roll_view()
            .frames
            .iter()
            .all(|f| f.status == FrameStatus::Ready)
    );
}

#[test]
fn export_size_after_rotate_90_and_crop() {
    let dir = Scratch::new("geometry");
    write_roll(&dir, 2, 120, 80);
    let mut s = analysed(&dir);
    let crop = CropRect {
        x: 0.1,
        y: 0.2,
        width: 0.5,
        height: 0.6,
    };
    let mut roll = s.roll.clone();
    roll.shared.geometry.crop = Some(crop);
    s.commit_roll(roll).unwrap();
    let mut geometry = s.roll.shared.geometry;
    geometry.rotate_degrees = 90;
    let o = FrameOverrides {
        geometry: Some(geometry),
        ..FrameOverrides::default()
    };
    s.commit_frame(1, o).unwrap();
    let out = dir.path("out");
    let report = s
        .export(
            &ExportRequest {
                scope: ExportScope::Frames { frames: vec![0, 1] },
                format: ExportFormat::Tiff16,
                out_dir: Some(out.to_string_lossy().into_owned()),
            },
            &mut |_| {},
        )
        .unwrap();
    assert_eq!((report.converted, report.failed), (2, 0));
    let size = |name: &str| {
        let d = negg_decode::decode(&out.join(name)).unwrap();
        (d.image.width(), d.image.height())
    };
    // Crop pixels: x 12..72, y 16..64 of 120 x 80.
    assert_eq!(size("f0.negg.tif"), (60, 48));
    assert_eq!(size("f1.negg.tif"), (48, 60));
    assert_eq!(geometry.output_size(120, 80), (48, 60));
    // The export rewrote the sidecars; the session still flushes over them.
    s.commit_frame(0, grade(100.0)).unwrap();
    s.flush().unwrap();
}

/// Timing on a 24 MP synthetic frame (Ruling 83): opening the frame, a
/// level-0 tile and a fit-level tile. Reported, not gated. Run with
/// `cargo test --release -p negg-session -- --ignored --nocapture`.
#[test]
#[ignore]
fn timing_24mp() {
    let dir = Scratch::new("timing");
    write_negative(&dir.path("big.tif"), 6000, 4000, [0.25, 0.55, 0.75]);
    let t = Instant::now();
    let mut s = analysed(&dir);
    eprintln!("open and pass 1: {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
    let t = Instant::now();
    let view = s.frame_view(0).unwrap();
    eprintln!(
        "open frame ({} levels): {:.0} ms",
        view.levels,
        t.elapsed().as_secs_f64() * 1e3
    );
    let t = Instant::now();
    let tile = s.render_tile(0, 0, 3, 3).unwrap();
    assert_eq!(tile.len(), TILE_HEADER_LEN + 512 * 512 * 4);
    eprintln!("level 0 tile: {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
    let t = Instant::now();
    s.render_tile(0, view.levels - 1, 0, 0).unwrap();
    eprintln!("fit-level tile: {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
}

#[test]
fn a_frame_batch_would_refuse_is_failed_and_never_written() {
    let dir = Scratch::new("blocked");
    write_negative(&dir.path("a.tif"), 48, 32, [0.25, 0.55, 0.75]);
    write_negative(&dir.path("a.tiff"), 48, 32, [0.25, 0.55, 0.75]);
    write_negative(&dir.path("roll.tif"), 48, 32, [0.25, 0.55, 0.75]);
    let mut s = analysed(&dir);
    let view = s.roll_view();
    let status: Vec<FrameStatus> = view.frames.iter().map(|f| f.status).collect();
    assert_eq!(
        status,
        [FrameStatus::Ready, FrameStatus::Failed, FrameStatus::Failed]
    );
    assert!(matches!(s.frame_view(1), Err(SessionError::Invalid(_))));
    assert!(matches!(
        s.commit_frame(2, grade(120.0)),
        Err(SessionError::ReadOnly(2))
    ));
    assert_eq!(s.pool.as_ref().unwrap().frames.len(), 1);
}

/// A film profile with a gamma table, measured with no rig.
fn write_film_profile(path: &Path) {
    use negg_types::profile::{FilmMethod, FilmProfile, GammaTable, StockTable};
    let profile = FilmProfile {
        schema_version: 1,
        name: "test-stock".into(),
        source: "synthetic".into(),
        measured_on: "2026-02-03".into(),
        method: FilmMethod::GreyCardRoll,
        residual_rms: 0.01,
        rig: None,
        stock: StockTable {
            name: "Test 400".into(),
            process: "C-41".into(),
            iso: 400,
        },
        base: None,
        dmax: None,
        gamma: Some(GammaTable {
            gamma: [1.7, 1.65, 1.6],
            ratio: [1.03, 1.0, 0.97],
            offset: [0.01, 0.0, -0.02],
            gamma_source: "neutral_fit".into(),
            n_patches: 3,
            source: "grey card".into(),
        }),
    };
    negg_io::write_film_profile(path, &profile).unwrap();
}

/// Runs `negg batch` on the roll and returns the bytes of `names` beside
/// the sources afterwards.
fn batch_sidecars(dir: &Scratch, names: &[&str]) -> Vec<Vec<u8>> {
    let req = crate::flow::OutputRequest {
        out_dir: Some(dir.path("batch-out")),
        ..crate::flow::OutputRequest::default()
    };
    crate::flow::batch_dir(&dir.0, &req, &mut |_| {}).unwrap();
    names
        .iter()
        .map(|n| std::fs::read(dir.path(n)).unwrap())
        .collect()
}

#[test]
fn estimates_are_written_only_on_the_final_pool() {
    let dir = Scratch::new("final-pool");
    write_roll(&dir, 3, 96, 64);
    write_film_profile(&dir.path("stock.film.negg.toml"));
    let mut s = analysed(&dir);
    s.commit_frame(0, grade(120.0)).unwrap();
    s.commit_frame(2, grade(90.0)).unwrap();
    let mut roll = s.roll.clone();
    roll.shared.profiles.film = Some(
        dir.path("stock.film.negg.toml")
            .to_string_lossy()
            .into_owned(),
    );
    assert!(!s.commit_roll(roll).unwrap().analysed);
    // Pass 1 reaches frames 0 and 1 only; a flush now (on close) writes no
    // estimates, since there is no final pool yet.
    s.analyse_next().unwrap();
    s.analyse_next().unwrap();
    assert!(matches!(s.frames[0].pass1, Some(Ok(_))));
    s.flush().unwrap();
    for name in ["f0.negg.json", "f2.negg.json"] {
        let side = negg_io::read_sidecar(&dir.path(name)).unwrap();
        assert_eq!(side.estimates, None, "{name}");
    }
    assert!(
        !s.is_dirty(),
        "nothing more to write until the pool is final"
    );
    // After pass 1 and its pool the next flush writes the estimates, and
    // the sidecars are exactly what `negg batch` writes.
    s.analyse_all().unwrap();
    assert!(s.is_dirty());
    s.flush().unwrap();
    assert!(!s.is_dirty());
    let names = ["f0.negg.json", "f2.negg.json", ROLL_SIDECAR_NAME];
    let flushed: Vec<Vec<u8>> = names
        .iter()
        .map(|n| std::fs::read(dir.path(n)).unwrap())
        .collect();
    let side = negg_io::read_sidecar(&dir.path("f0.negg.json")).unwrap();
    assert_eq!(side.estimates, s.frame_view(0).unwrap().estimates);
    assert!(side.estimates.is_some());
    assert!(batch_sidecars(&dir, &names) == flushed);
}

#[test]
fn a_frame_found_stale_in_a_flush_is_pooled_before_estimates_are_written() {
    let dir = Scratch::new("stale-pool");
    write_roll(&dir, 3, 96, 64);
    let mut s = analysed(&dir);
    s.commit_frame(0, grade(120.0)).unwrap();
    s.commit_frame(1, grade(110.0)).unwrap();
    s.flush().unwrap();
    // Frames 0 and 1 are edited, then someone locks frame 0's film base
    // to another value on disk before the flush.
    s.commit_frame(0, grade(125.0)).unwrap();
    s.commit_frame(1, grade(100.0)).unwrap();
    let path = dir.path("f0.negg.json");
    let mut side = negg_io::read_sidecar(&path).unwrap();
    side.overrides.film.film_base = Some(ChannelEstimate::Fixed {
        value: [0.6, 0.9, 1.1],
    });
    negg_io::write_sidecar(&path, &side).unwrap();
    assert!(matches!(s.flush(), Err(SessionError::Stale(_))));
    assert_eq!(s.frames[0].overrides, side.overrides, "reloaded from disk");
    // Frame 1's sidecar has its estimates on the pool with frame 0's new
    // overrides, the pool the roll sidecar records.
    let one = negg_io::read_sidecar(&dir.path("f1.negg.json")).unwrap();
    assert_eq!(one.estimates, s.frame_view(1).unwrap().estimates);
    let roll = negg_io::read_roll_sidecar(&dir.path(ROLL_SIDECAR_NAME)).unwrap();
    assert_eq!(roll.pool.as_ref(), s.pool.as_ref());
    assert_eq!(
        roll.pool.unwrap().frames[0].dmin_role,
        PoolRole::Excluded,
        "the pool saw frame 0's lock"
    );
    // Frame 2, never edited, was brought up to date too (Ruling 139).
    let names = ["f1.negg.json", "f2.negg.json", ROLL_SIDECAR_NAME];
    let flushed: Vec<Vec<u8>> = names
        .iter()
        .map(|n| std::fs::read(dir.path(n)).unwrap())
        .collect();
    let batch = batch_sidecars(&dir, &names);
    for (k, n) in names.iter().enumerate() {
        assert!(
            batch[k] == flushed[k],
            "{n}:\n{}\n{}",
            String::from_utf8_lossy(&flushed[k]),
            String::from_utf8_lossy(&batch[k])
        );
    }
}

#[test]
fn every_view_says_when_a_re_pool_is_pending() {
    let dir = Scratch::new("pending-views");
    write_roll(&dir, 2, 48, 32);
    let mut s = analysed(&dir);
    let view = s.lock_frame(0, LockWhich::default()).unwrap();
    assert!(view.pool_pending && view.history.pool_pending);
    assert!(s.roll_view().pool_pending);
    s.analyse_all().unwrap();
    let h = s.undo().unwrap();
    assert!(h.pool_pending);
    s.analyse_all().unwrap();
    let view = s.commit_frame(1, grade(100.0)).unwrap();
    assert!(!view.pool_pending && !view.history.pool_pending);
    assert!(s.redo().unwrap().pool_pending == s.roll_view().pool_pending);
}

/// A synthetic negative of `w` by `h` film (the pattern of
/// [`write_negative`]) inside an opaque surround `margin` pixels wide: a
/// film holder in the shot, transmittance 0.
fn write_negative_in_holder(path: &Path, w: usize, h: usize, margin: usize, dmin: [f32; 3]) {
    let border = (w.min(h) / 10).max(2);
    let (ow, oh) = (w + 2 * margin, h + 2 * margin);
    let mut data = vec![0u16; ow * oh * 3];
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
            let i = ((y + margin) * ow + x + margin) * 3;
            for (c, d) in dmin.into_iter().enumerate() {
                let transmittance = 10f32.powf(-(d + 1.6 * t));
                data[i + c] = (transmittance.powf(1.0 / 2.2) * 65535.0).round() as u16;
            }
        }
    }
    negg_io::write_tiff16(path, ow, oh, &data, &negg_io::TiffOptions::default()).unwrap();
}

/// A roll of `n` held frames of `w` by `h` film with a `margin` surround.
fn write_held_roll(dir: &Scratch, n: usize, w: usize, h: usize, margin: usize) {
    for i in 0..n {
        let d = 0.01 * i as f32;
        write_negative_in_holder(
            &dir.path(&format!("f{i}.tif")),
            w,
            h,
            margin,
            [0.25 + d, 0.55 + d, 0.75 + d],
        );
    }
}

/// The analysis area of the film in a held frame, with no buffer.
fn film_area(w: usize, h: usize, margin: usize) -> CropRect {
    let (ow, oh) = ((w + 2 * margin) as f32, (h + 2 * margin) as f32);
    CropRect {
        x: margin as f32 / ow,
        y: margin as f32 / oh,
        width: w as f32 / ow,
        height: h as f32 / oh,
    }
}

#[test]
fn the_negative_is_shown_before_any_analysis() {
    let dir = Scratch::new("negative");
    write_roll(&dir, 2, 96, 64);
    let before = listing(&dir.0);
    let mut s = Session::open_roll(&dir.0).unwrap();
    assert!(matches!(s.frame_view(0), Err(SessionError::Busy)));
    assert!(matches!(s.thumbnail_jpeg(0), Err(SessionError::Busy)));

    let thumb = s.negative_thumbnail_jpeg(1).unwrap();
    assert_eq!(&thumb[..2], &[0xFF, 0xD8], "a JPEG");
    let view = s.negative_view(0).unwrap();
    assert_eq!((view.width, view.height, view.levels), (96, 64, 1));
    assert_eq!(view.source, "f0.tif");
    let tile = s.negative_tile(0, 0, 0, 0).unwrap();
    let (header, rgba) = parse_tile(&tile).unwrap();
    assert_eq!((header.width, header.height, header.frame), (96, 64, 0));
    // Not inverted: the clear base of the border is lighter than the
    // densest part of the ramp at the right.
    let at = |x: usize, y: usize| rgba[(y * 96 + x) * 4 + 1];
    assert!(at(2, 32) > at(85, 32), "{} {}", at(2, 32), at(85, 32));
    assert!(s.negative_tile(0, 1, 0, 0).is_err());
    assert!(s.negative_tile(0, 0, 1, 0).is_err());
    assert!(matches!(s.negative_view(9), Err(SessionError::NoFrame(9))));
    let roll = s.roll_view();
    assert!(!roll.analysed);
    assert!(roll.frames.iter().all(|f| f.status == FrameStatus::Pending));
    assert_eq!(
        listing(&dir.0),
        before,
        "showing the negative writes nothing"
    );

    // Pass 1 still runs, and the negative is the same from its grid.
    s.analyse_all().unwrap();
    assert_eq!(s.negative_thumbnail_jpeg(1).unwrap(), thumb);
    let mut fresh = analysed(&dir);
    assert_eq!(fresh.negative_thumbnail_jpeg(1).unwrap(), thumb);
    let fresh_tile = fresh.negative_tile(0, 0, 0, 0).unwrap();
    assert_eq!(parse_tile(&fresh_tile).unwrap().1, rgba);
}

#[test]
fn an_already_open_negative_frame_is_busy_to_frame_view_before_analysis() {
    let dir = Scratch::new("negative-busy");
    write_roll(&dir, 2, 96, 64);
    let mut s = Session::open_roll(&dir.0).unwrap();
    s.negative_view(0).unwrap();
    assert!(matches!(s.frame_view(0), Err(SessionError::Busy)));
    assert!(matches!(s.render_tile(0, 0, 0, 0), Err(SessionError::Busy)));
    s.negative_view(0).unwrap();

    s.analyse_all().unwrap();
    let view = s.frame_view(0).unwrap();
    assert!(view.width > 0 && view.height > 0, "a full view");
}

#[test]
fn a_roll_analysis_area_set_before_pass_1_keeps_the_holder_out() {
    let (w, h, m) = (96, 64, 12);
    let dir = Scratch::new("area");
    write_held_roll(&dir, 3, w, h, m);

    // Without an analysis area every frame is pinned at the cap: flagged,
    // and no gamma pool.
    let mut plain = analysed(&dir);
    let roll = plain.roll_view();
    assert!(
        roll.frames.iter().all(|f| f.needs_analysis_area),
        "{roll:?}"
    );
    let v = plain.frame_view(0).unwrap();
    assert!(v.needs_analysis_area);
    let pool = v.pool.unwrap();
    assert!(pool.needs_analysis_area);
    assert_eq!(pool.gamma_role, PoolRole::Unused);
    assert_eq!(
        v.estimates.unwrap().gamma_source,
        negg_types::GammaSource::Range
    );

    // The area set before pass 1: nothing flagged, the frames pool.
    let area = film_area(w, h, m);
    let mut before = Session::open_roll(&dir.0).unwrap();
    let rv = before.set_roll_analysis_area(Some(area)).unwrap();
    assert_eq!(rv.roll.shared.geometry.analysis_area, Some(area));
    assert!(!rv.analysed);
    before.analyse_all().unwrap();
    let roll = before.roll_view();
    assert!(
        roll.frames.iter().all(|f| !f.needs_analysis_area),
        "{roll:?}"
    );
    let views: Vec<FrameView> = (0..3).map(|i| before.frame_view(i).unwrap()).collect();
    for v in &views {
        assert!(!v.needs_analysis_area);
        assert_eq!(v.pool.as_ref().unwrap().gamma_role, PoolRole::Inlier);
        assert_eq!(
            v.estimates.unwrap().gamma_source,
            negg_types::GammaSource::Roll
        );
        // The export keeps the whole frame: no crop.
        assert_eq!(
            (v.width, v.height),
            ((w + 2 * m) as u32, (h + 2 * m) as u32)
        );
        assert_eq!(v.settings.geometry.crop, None);
    }

    // Set during pass 1 (one frame analysed) or after it, through the
    // Ruling 81 path, the result is the same.
    let mut during = Session::open_roll(&dir.0).unwrap();
    during.analyse_next().unwrap();
    during.set_roll_analysis_area(Some(area)).unwrap();
    during.analyse_all().unwrap();
    plain.set_roll_analysis_area(Some(area)).unwrap();
    assert!(plain.roll_view().pool_pending);
    plain.analyse_all().unwrap();
    for i in 0..3u32 {
        let want = &views[i as usize];
        for s in [&mut during, &mut plain] {
            let got = s.frame_view(i).unwrap();
            assert_eq!(got.estimates, want.estimates, "frame {i}");
            assert_eq!(got.pool, want.pool, "frame {i}");
            assert!(!got.needs_analysis_area);
        }
    }
    // Undo after pass 1 takes the area away and the flags come back.
    plain.undo().unwrap();
    plain.analyse_all().unwrap();
    assert!(
        plain
            .roll_view()
            .frames
            .iter()
            .all(|f| f.needs_analysis_area)
    );
    // An invalid area is refused with nothing changed.
    let bad = CropRect {
        x: 0.5,
        y: 0.5,
        width: 0.6,
        height: 0.1,
    };
    assert!(matches!(
        before.set_roll_analysis_area(Some(bad)),
        Err(SessionError::Invalid(_))
    ));
    assert_eq!(
        before.roll_view().roll.shared.geometry.analysis_area,
        Some(area)
    );
}

#[test]
fn a_frame_analysis_area_override_reanalyses_that_frame() {
    let (w, h, m) = (96, 64, 12);
    let dir = Scratch::new("frame-area");
    write_held_roll(&dir, 3, w, h, m);
    let mut s = analysed(&dir);
    let before = s.frame_view(0).unwrap();
    assert!(before.needs_analysis_area);
    let pinned_dmax = before.pool.as_ref().unwrap().dmax;
    assert!(negg_types::dmax_pinned(&pinned_dmax));

    let o = FrameOverrides {
        geometry: Some(Geometry {
            analysis_area: Some(film_area(w, h, m)),
            analysis_buffer: 0.0,
            ..Geometry::default()
        }),
        ..FrameOverrides::default()
    };
    let v = s.commit_frame(0, o).unwrap();
    // The frame's pool input was rebuilt from its cached grid at once, and
    // the pool is pending.
    assert!(v.pool_pending);
    assert!(!v.needs_analysis_area);
    s.analyse_all().unwrap();
    let after = s.frame_view(0).unwrap();
    assert!(!after.needs_analysis_area);
    let record = after.pool.as_ref().unwrap();
    assert!(!record.needs_analysis_area);
    assert!(!negg_types::dmax_pinned(&record.dmax), "{record:?}");
    assert_ne!(
        after.estimates.unwrap().dmax,
        before.estimates.unwrap().dmax
    );
    assert_ne!(after.estimates, before.estimates);
    // Its export framing is untouched: no crop.
    assert_eq!(after.settings.geometry.crop, None);
    // The other frames keep the roll's (absent) area and stay flagged.
    for i in 1..3 {
        assert!(s.frame_view(i).unwrap().needs_analysis_area, "frame {i}");
    }
}

#[test]
fn browsing_a_folder_never_batched_writes_nothing() {
    let dir = Scratch::new("browse");
    write_roll(&dir, 3, 96, 64);
    let before = listing(&dir.0);
    let mut s = Session::open_roll(&dir.0).unwrap();
    s.negative_thumbnail_jpeg(0).unwrap();
    s.negative_view(1).unwrap();
    s.analyse_all().unwrap();
    s.frame_view(2).unwrap();
    s.thumbnail_jpeg(2).unwrap();
    s.flush().unwrap();
    drop(s);
    assert_eq!(listing(&dir.0), before, "Ruling 141: no roll.negg.json");
}

#[test]
fn an_out_of_date_roll_sidecar_is_rewritten_after_the_analysis() {
    let dir = Scratch::new("out-of-date");
    write_roll(&dir, 3, 96, 64);
    let names = [
        "f0.negg.json",
        "f1.negg.json",
        "f2.negg.json",
        ROLL_SIDECAR_NAME,
    ];
    let batched = batch_sidecars(&dir, &names);
    // Up to date: browsing and flushing writes nothing.
    let before = listing(&dir.0);
    let mut s = analysed(&dir);
    s.flush().unwrap();
    assert_eq!(listing(&dir.0), before, "a batched roll is up to date");
    drop(s);
    // The roll sidecar loses its pool (as an older run might leave it):
    // after the analysis the flush writes the pool `negg batch` writes.
    let path = dir.path(ROLL_SIDECAR_NAME);
    let mut roll = negg_io::read_roll_sidecar(&path).unwrap();
    roll.pool = None;
    negg_io::write_roll_sidecar(&path, &roll).unwrap();
    let mut s = Session::open_roll(&dir.0).unwrap();
    s.flush().unwrap();
    assert_ne!(
        std::fs::read(&path).unwrap(),
        batched[3],
        "not before pass 1"
    );
    s.analyse_all().unwrap();
    s.flush().unwrap();
    let now: Vec<Vec<u8>> = names
        .iter()
        .map(|n| std::fs::read(dir.path(n)).unwrap())
        .collect();
    assert!(now == batched);
}

#[test]
fn pass_1_reuses_the_decode_of_the_negative_view() {
    let dir = Scratch::new("reuse");
    write_roll(&dir, 3, 96, 64);
    let mut fresh = analysed(&dir);
    let mut s = Session::open_roll(&dir.0).unwrap();
    s.negative_thumbnail_jpeg(0).unwrap();
    s.negative_tile(1, 0, 0, 0).unwrap();
    // Frames 0 and 1 are not decoded again: pass 1 succeeds without their
    // sources, with the very data a fresh pass 1 makes.
    let moved = dir.path("moved");
    std::fs::create_dir_all(&moved).unwrap();
    for name in ["f0.tif", "f1.tif"] {
        std::fs::rename(dir.path(name), moved.join(name)).unwrap();
    }
    s.analyse_all().unwrap();
    assert!(
        s.roll_view()
            .frames
            .iter()
            .all(|f| f.status == FrameStatus::Ready)
    );
    assert_eq!(s.pool, fresh.pool);
    for i in 0..3 {
        assert_eq!(
            s.thumbnail_jpeg(i).unwrap(),
            fresh.thumbnail_jpeg(i).unwrap(),
            "frame {i}"
        );
    }
    // New profiles drop what the negative view kept: frame 0 is decoded
    // again (and fails without its source).
    let mut s = Session::open_roll(&dir.0).unwrap();
    for name in ["f0.tif", "f1.tif"] {
        std::fs::rename(moved.join(name), dir.path(name)).unwrap();
    }
    s.negative_thumbnail_jpeg(0).unwrap();
    s.set_roll_analysis_area(None).unwrap();
    s.analyse_all().unwrap();
    let mut roll = s.roll.clone();
    roll.shared.profiles.film = Some(dir.path("missing.film.negg.toml").to_string_lossy().into());
    s.commit_roll(roll).unwrap();
    assert!(
        s.frames
            .iter()
            .all(|f| f.pre.is_none() && f.pass1.is_none())
    );
}

#[test]
fn a_frame_that_failed_to_decode_never_forces_a_re_pool() {
    let dir = Scratch::new("bad-decode");
    write_roll(&dir, 2, 96, 64);
    std::fs::write(dir.path("f2.tif"), b"not a tiff").unwrap();
    let mut s = analysed(&dir);
    assert_eq!(s.roll_view().frames[2].status, FrameStatus::Failed);
    let view = s.commit_frame(2, grade(120.0)).unwrap();
    assert!(!view.pool_pending, "nothing of it is in the pool");
    let mut o = grade(120.0);
    o.colour.auto_colour = Some(false);
    let view = s.commit_frame(2, o).unwrap();
    assert!(!view.pool_pending);
    let mut roll = s.roll.clone();
    roll.shared.print.grade = 100.0;
    assert!(!s.commit_roll(roll).unwrap().pool_pending);
}

#[test]
fn an_edit_of_a_frame_changed_on_disk_is_refused_up_front() {
    let dir = Scratch::new("refuse-edit");
    write_roll(&dir, 2, 96, 64);
    let mut s = analysed(&dir);
    s.commit_frame(0, grade(130.0)).unwrap();
    s.flush().unwrap();
    let path = dir.path("f0.negg.json");
    let mut side = negg_io::read_sidecar(&path).unwrap();
    side.overrides.print.grade = Some(60.0);
    negg_io::write_sidecar(&path, &side).unwrap();
    let err = s.commit_frame(0, grade(140.0)).unwrap_err();
    assert!(
        matches!(&err, SessionError::Stale(p) if *p == path),
        "{err}"
    );
    assert_eq!(s.frames[0].overrides, side.overrides, "what is on disk");
    assert!(matches!(
        s.preview_frame(0, grade(90.0)),
        Err(SessionError::ReadOnly(0))
    ));
    assert_eq!(negg_io::read_sidecar(&path).unwrap(), side);
    s.flush().unwrap();
    assert_eq!(negg_io::read_sidecar(&path).unwrap(), side, "never written");
}

#[test]
fn an_area_that_leaves_out_the_rebate_warns() {
    let (w, h) = (120, 80);
    let dir = Scratch::new("rebate");
    write_roll(&dir, 3, w, h);
    let mut s = analysed(&dir);
    assert!(s.pool.as_ref().unwrap().dmin_from_border(), "a Border roll");
    assert!(s.roll_view().pool_warning.is_none());
    // An area inside the clear border: the film edge is left out.
    let inner = CropRect {
        x: 0.25,
        y: 0.25,
        width: 0.5,
        height: 0.5,
    };
    s.set_roll_analysis_area(Some(inner)).unwrap();
    s.analyse_all().unwrap();
    assert!(!s.pool.as_ref().unwrap().dmin_from_border());
    let view = s.roll_view();
    assert_eq!(view.pool_warning.as_deref(), Some(BORDER_LOST_WARNING));
    assert!(s.warnings().iter().any(|w| w == BORDER_LOST_WARNING));
    assert!(BORDER_LOST_WARNING.contains("clear film edge"));
    // The whole film again: the pool is on the edge and the warning goes.
    s.set_roll_analysis_area(Some(CropRect {
        x: 0.0,
        y: 0.0,
        width: 1.0,
        height: 1.0,
    }))
    .unwrap();
    s.analyse_all().unwrap();
    assert!(s.pool.as_ref().unwrap().dmin_from_border());
    assert!(s.roll_view().pool_warning.is_none());
}

#[test]
fn export_drops_the_open_frame_and_the_plans() {
    let dir = Scratch::new("export-memory");
    write_roll(&dir, 2, 96, 64);
    let mut s = analysed(&dir);
    s.render_tile(0, 0, 0, 0).unwrap();
    assert!(s.open.is_some());
    s.export(
        &ExportRequest {
            scope: ExportScope::Roll,
            format: ExportFormat::Jpeg,
            out_dir: Some(dir.path("out").to_string_lossy().into_owned()),
        },
        &mut |_| {},
    )
    .unwrap();
    assert!(s.open.is_none());
    assert!(s.frames.iter().all(|f| f.plans.is_empty()));
}
