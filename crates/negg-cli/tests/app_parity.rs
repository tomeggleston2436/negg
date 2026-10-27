// SPDX-License-Identifier: Apache-2.0

//! Gate 3, the automatic half: the app's engine API
//! ([`negg_session::Session`]) and the `negg` binary produce byte-identical
//! files for the same settings. A synthetic 6-frame float TIFF roll is
//! edited through the session (roll crop, analysis buffer and output space;
//! grade, density, CMY, toe, shoulder, black point compensation, rotate 90,
//! flip, a frame crop override, lock, undo and redo), exported as TIFF16
//! and JPEG through `Session::export`, then `negg batch` and `negg convert`
//! run on the same directory into other output directories. Every output
//! and every sidecar must be equal byte for byte, with no masking. The
//! preview is checked against the export too: level-0 tiles equal the
//! frame's render, and the estimates `frame_view` shows are the ones the
//! export sidecar records.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::{TempDir, negative_linear, write_float_tiff};
use negg_session::{
    ExportFormat, ExportReport, ExportRequest, ExportScope, FrameView, LockWhich, RollPresetScope,
    Session, WebOutput, build_context, parse_tile, prepare_frame, render_in_roll,
};
use negg_types::settings::OutputSpace;
use negg_types::{CropRect, FrameOverrides, Geometry, RollSettings, builtin_presets};

const W: usize = 576;
const H: usize = 384;

/// Six frames with slightly different bases and contrasts, one denser.
fn write_roll(dir: &Path) {
    let frames = [
        ([0.20, 0.50, 0.70], [1.60, 1.70, 1.80]),
        ([0.21, 0.51, 0.71], [1.55, 1.65, 1.80]),
        ([0.22, 0.52, 0.72], [1.60, 1.70, 1.75]),
        ([0.20, 0.51, 0.70], [1.50, 1.60, 1.70]),
        ([0.21, 0.50, 0.72], [1.65, 1.70, 1.85]),
        ([0.45, 0.75, 0.95], [1.60, 1.70, 1.80]),
    ];
    for (i, (dmin, gamma)) in frames.into_iter().enumerate() {
        let data = negative_linear(W, H, dmin, gamma, |x, y| {
            // A grey patch per frame, at a different place each time.
            let (px, py) = (60 + 70 * i, 120 + 20 * i);
            (x >= px && x < px + 40 && y >= py && y < py + 40).then_some([0.3; 3])
        });
        write_float_tiff(&dir.join(format!("frame{i}.tif")), W, H, &data);
    }
}

fn negg(args: &[&str]) {
    let o = Command::new(env!("CARGO_BIN_EXE_negg"))
        .args(args)
        .output()
        .expect("run negg");
    assert_eq!(
        o.status.code(),
        Some(0),
        "negg {args:?}\n{}\n{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
}

/// Every file of `dir` (not recursive) by name, with its bytes.
fn files(dir: &Path, keep: impl Fn(&str) -> bool) -> BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_file())
        .filter_map(|p| {
            let name = p.file_name()?.to_string_lossy().into_owned();
            keep(&name).then(|| (name, std::fs::read(&p).unwrap()))
        })
        .collect()
}

/// The sidecars beside the sources, the roll sidecar included.
fn sidecars(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    files(dir, |n| n.ends_with(".negg.json"))
}

fn assert_same_files(app: &Path, cli: &Path, expected: usize) {
    let a = files(app, |_| true);
    let c = files(cli, |_| true);
    assert_eq!(a.len(), expected, "{app:?}: {:?}", a.keys());
    assert_eq!(
        a.keys().collect::<Vec<_>>(),
        c.keys().collect::<Vec<_>>(),
        "{app:?} and {cli:?} hold different files"
    );
    for (name, bytes) in &a {
        assert!(
            *bytes == c[name],
            "{name}: the app export and the CLI output differ"
        );
    }
}

fn export(
    session: &mut Session,
    scope: ExportScope,
    format: ExportFormat,
    out: &Path,
) -> ExportReport {
    export_web(session, scope, format, WebOutput::None, out)
}

fn export_web(
    session: &mut Session,
    scope: ExportScope,
    format: ExportFormat,
    web: WebOutput,
    out: &Path,
) -> ExportReport {
    let req = ExportRequest {
        scope,
        format,
        out_dir: Some(out.to_string_lossy().into_owned()),
        web,
    };
    let report = session.export(&req, &mut |_| {}).unwrap();
    assert_eq!(report.failed, 0, "{:?}", report.errors);
    report
}

fn with_grade(o: &FrameOverrides, grade: f32) -> FrameOverrides {
    let mut o = o.clone();
    o.print.grade = Some(grade);
    o
}

/// The level-0 tiles of `view`'s frame assembled into one RGB8 image.
fn level_0_rgb(session: &mut Session, view: &FrameView) -> Vec<u8> {
    let (w, h) = (view.width as usize, view.height as usize);
    let t = view.tile_size as usize;
    let mut rgb = vec![0u8; w * h * 3];
    for ty in 0..h.div_ceil(t) {
        for tx in 0..w.div_ceil(t) {
            let bytes = session
                .render_tile(view.index, 0, tx as u32, ty as u32)
                .unwrap();
            let (header, pixels) = parse_tile(&bytes).unwrap();
            let (tw, th) = (header.width as usize, header.height as usize);
            for y in 0..th {
                for x in 0..tw {
                    let src = &pixels[(y * tw + x) * 4..][..3];
                    let at = ((ty * t + y) * w + tx * t + x) * 3;
                    rgb[at..at + 3].copy_from_slice(src);
                }
            }
        }
    }
    rgb
}

#[test]
fn app_exports_equal_cli_outputs_byte_for_byte() {
    let tmp = TempDir::new("app-parity");
    let roll_dir = tmp.path("roll");
    write_roll(&roll_dir);
    let out = |name: &str| -> PathBuf { tmp.path(name) };

    let mut s = Session::open_roll(&roll_dir).unwrap();
    s.analyse_all().unwrap();
    assert!(s.roll_view().analysed);
    assert!(
        files(&roll_dir, |n| n.contains(".negg.")).is_empty(),
        "opening and analysing a roll writes nothing"
    );

    // Roll edits: crop, analysis buffer and the output space.
    let mut roll: RollSettings = s.roll_view().roll;
    roll.shared.geometry.crop = Some(CropRect {
        x: 0.04,
        y: 0.05,
        width: 0.92,
        height: 0.9,
    });
    roll.shared.geometry.analysis_buffer = 0.03;
    roll.shared.colour.output_space = OutputSpace::AdobeRgb;
    s.commit_roll(roll.clone()).unwrap();
    // A pool flag toggled and undone, then redone and undone again.
    let mut flag = roll.clone();
    flag.pool_colour = false;
    s.commit_roll(flag).unwrap();
    s.undo().unwrap();
    s.redo().unwrap();
    s.undo().unwrap();
    assert_eq!(s.roll_view().roll, roll);

    // Frame 0: grade, density and CMY.
    let mut o = FrameOverrides::default();
    o.print.grade = Some(130.0);
    o.print.exposure = Some(0.15);
    o.print.cmy = Some([0.04, -0.02, 0.03]);
    s.commit_frame(0, o).unwrap();
    // Frame 1: toe, shoulder and black point compensation.
    let mut o = FrameOverrides::default();
    o.print.toe = Some(8.0);
    o.print.shoulder = Some(4.0);
    o.print.black_point_compensation = Some(true);
    s.commit_frame(1, o).unwrap();
    // Frame 2: rotate 90 and flip, keeping the roll crop.
    let o = FrameOverrides {
        geometry: Some(Geometry {
            rotate_degrees: 90,
            flip_horizontal: true,
            ..roll.shared.geometry
        }),
        ..FrameOverrides::default()
    };
    s.commit_frame(2, o).unwrap();
    // Frame 3: its own crop and analysis buffer.
    let o = FrameOverrides {
        geometry: Some(Geometry {
            crop: Some(CropRect {
                x: 0.1,
                y: 0.15,
                width: 0.7,
                height: 0.6,
            }),
            analysis_buffer: 0.1,
            ..Geometry::default()
        }),
        ..FrameOverrides::default()
    };
    s.commit_frame(3, o).unwrap();
    // Frame 4: lock its Dmin, gamma and cast.
    let locked = s.lock_frame(4, LockWhich::default()).unwrap();
    assert!(locked.overrides.film.film_base.is_some());
    assert!(locked.overrides.colour.cast.is_some());
    // Frame 5: a drag, commits, undo and redo, and a commit that drops the
    // redo tail.
    let base = FrameOverrides::default();
    s.preview_frame(5, with_grade(&base, 175.0)).unwrap();
    s.commit_frame(5, with_grade(&base, 150.0)).unwrap();
    s.commit_frame(5, with_grade(&base, 160.0)).unwrap();
    s.undo().unwrap();
    s.redo().unwrap();
    s.undo().unwrap();
    let mut o = with_grade(&base, 150.0);
    o.print.exposure = Some(-0.1);
    let v5 = s.commit_frame(5, o.clone()).unwrap();
    assert!(!v5.history.can_redo);
    assert_eq!(v5.overrides, o);

    assert!(
        s.roll_view().pool_pending,
        "the edits left a re-pool pending"
    );
    s.flush().unwrap();
    assert!(!s.roll_view().pool_pending, "flush ran the pending re-pool");
    let flushed_roll = std::fs::read(roll_dir.join("roll.negg.json")).unwrap();
    // The flush wrote all six frame sidecars (the edited frames, and
    // any frame a roll edit made out of date); they must equal
    // what `negg batch` writes.
    let session_flushed = sidecars(&roll_dir);
    assert_eq!(session_flushed.len(), 7);
    let views: Vec<FrameView> = (0..6).map(|i| s.frame_view(i).unwrap()).collect();
    for v in &views {
        assert!(v.estimates.is_some(), "frame {} has no estimates", v.index);
    }

    // Level-0 tiles equal the frame's render, for the rotated
    // frame and the frame with its own crop. The reference is pass 2 of
    // `negg batch`: prepare, then render with the roll context and the
    // pool the session flushed.
    let flushed = negg_io::read_roll_sidecar(&roll_dir.join("roll.negg.json")).unwrap();
    let (ctx, _) = build_context(flushed.settings.clone(), &roll_dir).unwrap();
    let srgb =
        negg_cms::working_to_output(OutputSpace::Srgb, negg_cms::Intent::Perceptual).unwrap();
    for i in [2usize, 3] {
        let view = s.frame_view(i as u32).unwrap();
        assert_eq!(
            (view.width as usize, view.height as usize, view.levels),
            (W, H, 2)
        );
        let prepared = prepare_frame(&roll_dir.join(&view.source), None).unwrap();
        let render =
            render_in_roll(&prepared, &view.settings, &ctx, flushed.pool.as_ref()).unwrap();
        assert_eq!(Some(render.estimates), view.estimates);
        let tiles = level_0_rgb(&mut s, &view);
        assert!(
            tiles == srgb.apply_u8(&render.image),
            "frame {i}: level 0 tiles differ from the render"
        );
    }

    // Export through the session.
    let r = export(
        &mut s,
        ExportScope::Roll,
        ExportFormat::Tiff16,
        &out("app-tif"),
    );
    assert_eq!(r.converted, 6);
    export(
        &mut s,
        ExportScope::Roll,
        ExportFormat::Jpeg,
        &out("app-jpg"),
    );
    export(
        &mut s,
        ExportScope::Frame { frame: 2 },
        ExportFormat::Tiff16,
        &out("app-one"),
    );
    export(
        &mut s,
        ExportScope::Frames { frames: vec![3, 4] },
        ExportFormat::Jpeg,
        &out("app-two"),
    );

    // The session pooled exactly as `negg batch` pools: the roll sidecar
    // the batch export wrote equals the one the session flushed.
    assert!(
        std::fs::read(roll_dir.join("roll.negg.json")).unwrap() == flushed_roll,
        "the session's pool differs from the batch pool"
    );
    // The estimates shown are the estimates exported.
    for v in &views {
        let stem = v.source.trim_end_matches(".tif");
        let sidecar = negg_io::read_sidecar(&roll_dir.join(format!("{stem}.negg.json"))).unwrap();
        assert_eq!(sidecar.estimates, v.estimates, "frame {}", v.index);
        assert_eq!(sidecar.settings, v.settings, "frame {}", v.index);
        assert_eq!(sidecar.overrides, v.overrides, "frame {}", v.index);
    }
    // The cropped and rotated frame has the rotated size of its crop.
    let d = negg_decode::decode(&out("app-one").join("frame2.negg.tif")).unwrap();
    let crop = roll.shared.geometry.crop_pixels(W, H);
    assert_eq!(
        (d.image.width(), d.image.height()),
        (crop.3 - crop.1, crop.2 - crop.0)
    );
    let app_sidecars = sidecars(&roll_dir);
    assert_eq!(app_sidecars.len(), 7);

    // The CLI on the same directory, into other output directories.
    let roll_arg = roll_dir.to_str().unwrap();
    let arg = |p: PathBuf| p.to_string_lossy().into_owned();
    negg(&[
        "batch",
        roll_arg,
        "--format",
        "tiff16",
        "--out",
        &arg(out("cli-tif")),
    ]);
    negg(&[
        "batch",
        roll_arg,
        "--format",
        "jpeg",
        "--out",
        &arg(out("cli-jpg")),
    ]);
    let frame = |i: usize| arg(roll_dir.join(format!("frame{i}.tif")));
    negg(&[
        "convert",
        &frame(2),
        "--format",
        "tiff16",
        "--out",
        &arg(out("cli-one")),
    ]);
    for i in [3, 4] {
        negg(&[
            "convert",
            &frame(i),
            "--format",
            "jpeg",
            "--out",
            &arg(out("cli-two")),
        ]);
    }

    assert_same_files(&out("app-tif"), &out("cli-tif"), 6);
    assert_same_files(&out("app-jpg"), &out("cli-jpg"), 6);
    assert_same_files(&out("app-one"), &out("cli-one"), 1);
    assert_same_files(&out("app-two"), &out("cli-two"), 2);
    let cli_sidecars = sidecars(&roll_dir);
    assert_eq!(
        app_sidecars.keys().collect::<Vec<_>>(),
        cli_sidecars.keys().collect::<Vec<_>>()
    );
    for (name, bytes) in &app_sidecars {
        assert!(*bytes == cli_sidecars[name], "{name} differs");
    }
    // The sidecars the session itself flushed, before any export, are the
    // ones `negg batch` writes, byte for byte.
    assert_eq!(
        session_flushed.keys().collect::<Vec<_>>(),
        cli_sidecars.keys().collect::<Vec<_>>()
    );
    for (name, bytes) in &session_flushed {
        assert!(
            *bytes == cli_sidecars[name],
            "{name}: the session's flush differs from negg batch"
        );
    }

    // The session carries on after the exports: a new edit flushes over
    // the sidecars the flows rewrote.
    let mut o = views[0].overrides.clone();
    o.print.grade = Some(100.0);
    s.commit_frame(0, o).unwrap();
    s.flush().unwrap();
}

/// `negg` with `NEGG_PRESETS_DIR` set to `presets` (on the child only), so
/// that the user's own presets folder is never read.
fn negg_with_presets(presets: &Path, args: &[&str]) {
    let o = Command::new(env!("CARGO_BIN_EXE_negg"))
        .env("NEGG_PRESETS_DIR", presets)
        .args(args)
        .output()
        .expect("run negg");
    assert_eq!(
        o.status.code(),
        Some(0),
        "negg {args:?}\n{}\n{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
}

/// Copies the files of `from` (not recursive) into a new `to`.
fn copy_files(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let p = e.unwrap().path();
        if p.is_file() {
            std::fs::copy(&p, to.join(p.file_name().unwrap())).unwrap();
        }
    }
}

/// A preset applied in the app and on the command line: with
/// `scope`, the session's roll preset then a roll export against
/// `negg batch --preset Rich [--preset-all-frames]`; without, the
/// session's frame preset on frame 3 then its export against
/// `negg convert frame3.tif --preset Rich`. Both start from the same
/// flushed roll, where frames 1 and 2 have their own look and frame 2
/// other overrides too.
fn preset_parity(tag: &str, scope: Option<RollPresetScope>) {
    let tmp = TempDir::new(tag);
    let app_dir = tmp.path("app");
    let cli_dir = tmp.path("cli");
    let presets = tmp.path("presets");
    write_roll(&app_dir);
    let mut s = Session::open_roll(&app_dir).unwrap();
    s.analyse_all().unwrap();
    s.commit_frame(1, with_grade(&FrameOverrides::default(), 100.0))
        .unwrap();
    let mut o = FrameOverrides::default();
    o.print.toe = Some(8.0);
    o.print.exposure = Some(0.1);
    o.print.cmy = Some([0.02, 0.0, -0.01]);
    s.commit_frame(2, o).unwrap();
    s.flush().unwrap();
    copy_files(&app_dir, &cli_dir);

    let rich = builtin_presets()
        .into_iter()
        .find(|p| p.name == "Rich")
        .unwrap();
    let arg = |p: &Path| p.to_string_lossy().into_owned();
    let (app_out, cli_out) = (tmp.path("app-out"), tmp.path("cli-out"));
    let expected = match scope {
        Some(scope) => {
            if scope == RollPresetScope::FramesWithoutOwnLook {
                assert_eq!(s.frames_with_own_look(), [1, 2]);
            }
            s.apply_roll_preset(&rich, scope).unwrap();
            export(&mut s, ExportScope::Roll, ExportFormat::Tiff16, &app_out);
            let mut args = vec![
                "batch".to_owned(),
                arg(&cli_dir),
                "--preset".into(),
                "Rich".into(),
                "--format".into(),
                "tiff16".into(),
                "--out".into(),
                arg(&cli_out),
            ];
            if scope == RollPresetScope::AllFrames {
                args.push("--preset-all-frames".into());
            }
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            negg_with_presets(&presets, &args);
            6
        }
        None => {
            s.apply_frame_preset(3, &rich).unwrap();
            export(
                &mut s,
                ExportScope::Frame { frame: 3 },
                ExportFormat::Tiff16,
                &app_out,
            );
            negg_with_presets(
                &presets,
                &[
                    "convert",
                    &arg(&cli_dir.join("frame3.tif")),
                    "--preset",
                    "Rich",
                    "--format",
                    "tiff16",
                    "--out",
                    &arg(&cli_out),
                ],
            );
            1
        }
    };
    assert_same_files(&app_out, &cli_out, expected);
    let (a, c) = (sidecars(&app_dir), sidecars(&cli_dir));
    assert_eq!(a.keys().collect::<Vec<_>>(), c.keys().collect::<Vec<_>>());
    for (name, bytes) in &a {
        assert!(*bytes == c[name], "{name}: the app and the CLI differ");
    }
    // The preset took effect: frame 3 renders with Rich's look.
    let s3 = negg_io::read_sidecar(&app_dir.join("frame3.negg.json")).unwrap();
    assert_eq!(s3.settings.print.preset.as_deref(), Some("Rich"));
    assert_eq!(s3.settings.print.grade, rich.look.grade);
    // Frame 1 keeps its own grade only when frames keep their own look.
    let s1 = negg_io::read_sidecar(&app_dir.join("frame1.negg.json")).unwrap();
    let want = match scope {
        Some(RollPresetScope::AllFrames) => rich.look.grade,
        _ => 100.0,
    };
    assert_eq!(s1.settings.print.grade, want);
}

#[test]
fn app_roll_preset_equals_cli_byte_for_byte() {
    preset_parity("preset-own", Some(RollPresetScope::FramesWithoutOwnLook));
    preset_parity("preset-all", Some(RollPresetScope::AllFrames));
}

#[test]
fn app_frame_preset_equals_cli_byte_for_byte() {
    preset_parity("preset-frame", None);
}

/// The web output in the app and on the command line (ADR 0035): the
/// session's roll export with `web: Also` against `negg batch --web`, and
/// one frame with `web: Only` against `negg convert --web-only`, on a roll
/// whose output space is Adobe RGB (the web copies are sRGB all the same).
/// Images, web images and sidecars must be equal byte for byte.
#[test]
fn app_web_export_equals_cli_byte_for_byte() {
    let tmp = TempDir::new("web-parity");
    let app_dir = tmp.path("app");
    let cli_dir = tmp.path("cli");
    write_roll(&app_dir);
    let mut s = Session::open_roll(&app_dir).unwrap();
    s.analyse_all().unwrap();
    let mut roll: RollSettings = s.roll_view().roll;
    roll.shared.colour.output_space = OutputSpace::AdobeRgb;
    s.commit_roll(roll).unwrap();
    s.commit_frame(1, with_grade(&FrameOverrides::default(), 120.0))
        .unwrap();
    s.flush().unwrap();
    copy_files(&app_dir, &cli_dir);
    let arg = |p: &Path| p.to_string_lossy().into_owned();

    // The whole roll, originals and web copies.
    let (app_roll, cli_roll) = (tmp.path("app-roll"), tmp.path("cli-roll"));
    let r = export_web(
        &mut s,
        ExportScope::Roll,
        ExportFormat::Tiff16,
        WebOutput::Also,
        &app_roll,
    );
    assert_eq!(r.converted, 6);
    assert_eq!(r.outputs.len(), 12, "{:?}", r.outputs);
    for i in 0..6 {
        for p in [
            app_roll.join(format!("frame{i}.negg.tif")),
            app_roll.join("web").join(format!("frame{i}.negg.jpg")),
        ] {
            assert!(r.outputs.contains(&arg(&p)), "{p:?} not in {:?}", r.outputs);
        }
    }
    negg(&[
        "batch",
        &arg(&cli_dir),
        "--format",
        "tiff16",
        "--web",
        "--out",
        &arg(&cli_roll),
    ]);
    assert_same_files(&app_roll, &cli_roll, 6);
    assert_same_files(&app_roll.join("web"), &cli_roll.join("web"), 6);

    // One frame, the web copy only.
    let (app_one, cli_one) = (tmp.path("app-one"), tmp.path("cli-one"));
    let r = export_web(
        &mut s,
        ExportScope::Frame { frame: 3 },
        ExportFormat::Jpeg,
        WebOutput::Only,
        &app_one,
    );
    assert_eq!(
        r.outputs,
        [arg(&app_one.join("web").join("frame3.negg.jpg"))]
    );
    negg(&[
        "convert",
        &arg(&cli_dir.join("frame3.tif")),
        "--format",
        "jpeg",
        "--web-only",
        "--out",
        &arg(&cli_one),
    ]);
    assert_same_files(&app_one, &cli_one, 0);
    assert_same_files(&app_one.join("web"), &cli_one.join("web"), 1);

    let (a, c) = (sidecars(&app_dir), sidecars(&cli_dir));
    assert_eq!(a.len(), 7);
    assert_eq!(a.keys().collect::<Vec<_>>(), c.keys().collect::<Vec<_>>());
    for (name, bytes) in &a {
        assert!(*bytes == c[name], "{name}: the app and the CLI differ");
    }
}
