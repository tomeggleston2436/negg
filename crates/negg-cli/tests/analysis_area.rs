// SPDX-License-Identifier: Apache-2.0

//! End-to-end tests for the analysis area and the holder guard (Rulings
//! 133 and 134): `--analysis-area` limits what the analysis sees without
//! cropping the export, frames whose Dmax is pinned at the density cap are
//! flagged and kept out of the gamma and cast pools, and the app's session
//! with the roll's analysis area set before pass 1 writes the very bytes
//! `negg batch --analysis-area` writes. Synthetic data only.

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Output};

use common::{TempDir, negative_linear, read_json, write_float_tiff};
use negg_session::{ExportFormat, ExportRequest, ExportScope, Session};
use negg_types::CropRect;

/// Film size and the opaque surround of a held frame.
const FW: usize = 256;
const FH: usize = 192;
const M: usize = 32;
/// The film inside a held frame, as `--analysis-area` takes it.
const AREA: &str = "0.1,0.125,0.8,0.75";
const AREA_RECT: CropRect = CropRect {
    x: 0.1,
    y: 0.125,
    width: 0.8,
    height: 0.75,
};

fn negg(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_negg"))
        .args(args)
        .output()
        .expect("run negg")
}

fn ok(o: &Output) {
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}\n{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// The film's linear transmittance, `FW` by `FH`.
fn film(dmin: [f32; 3], gamma: [f32; 3]) -> Vec<f32> {
    negative_linear(FW, FH, dmin, gamma, |_, _| None)
}

/// Writes the film as it is (a clean frame).
fn write_clean(path: &Path, dmin: [f32; 3], gamma: [f32; 3]) {
    write_float_tiff(path, FW, FH, &film(dmin, gamma));
}

/// Writes the film inside an opaque surround `M` pixels wide
/// (transmittance 0: a film holder in the shot).
fn write_held(path: &Path, dmin: [f32; 3], gamma: [f32; 3]) {
    let f = film(dmin, gamma);
    let (w, h) = (FW + 2 * M, FH + 2 * M);
    let mut data = vec![0.0f32; w * h * 3];
    for y in 0..FH {
        let src = &f[y * FW * 3..(y + 1) * FW * 3];
        let at = ((y + M) * w + M) * 3;
        data[at..at + FW * 3].copy_from_slice(src);
    }
    write_float_tiff(path, w, h, &data);
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Width and height of a TIFF output.
fn tiff_size(path: &Path) -> (usize, usize) {
    let d = negg_decode::decode(path).unwrap();
    (d.image.width(), d.image.height())
}

#[test]
fn convert_with_an_analysis_area_matches_the_clean_frame_at_full_size() {
    let tmp = TempDir::new("area-convert");
    let (dmin, gamma) = ([0.2, 0.5, 0.7], [1.6, 1.7, 1.8]);
    write_clean(&tmp.path("clean/f.tif"), dmin, gamma);
    write_held(&tmp.path("held/f.tif"), dmin, gamma);

    // The clean frame analysed over its whole area (buffered as any area).
    let o = negg(&[
        "convert",
        &s(&tmp.path("clean/f.tif")),
        "--analysis-area",
        "0,0,1,1",
        "--out",
        &s(&tmp.path("out-clean")),
        "--dump-stats",
    ]);
    ok(&o);
    assert!(
        !stdout(&o).contains("needs analysis area"),
        "{}",
        stdout(&o)
    );
    // The held frame without an area: Dmax pinned, flagged.
    let o = negg(&[
        "convert",
        &s(&tmp.path("held/f.tif")),
        "--out",
        &s(&tmp.path("out-plain")),
        "--dump-stats",
    ]);
    ok(&o);
    assert!(
        stdout(&o).contains("needs analysis area: Dmax at the density cap"),
        "{}",
        stdout(&o)
    );
    // The held frame with the area on its film.
    let o = negg(&[
        "convert",
        &s(&tmp.path("held/f.tif")),
        "--analysis-area",
        AREA,
        "--out",
        &s(&tmp.path("out-held")),
        "--dump-stats",
    ]);
    ok(&o);
    assert!(
        !stdout(&o).contains("needs analysis area"),
        "{}",
        stdout(&o)
    );

    let clean = read_json(&tmp.path("clean/f.negg.json"));
    let held = read_json(&tmp.path("held/f.negg.json"));
    assert_eq!(held["estimates"], clean["estimates"]);
    let g = &held["settings"]["geometry"];
    assert_eq!(g["crop"], serde_json::Value::Null);
    assert_eq!(g["analysis_area"]["y"], serde_json::json!(0.125));
    assert_eq!(held["schema_version"], serde_json::json!(5));
    // The export keeps the whole frame, holder included.
    assert_eq!(
        tiff_size(&tmp.path("out-held/f.negg.tif")),
        (FW + 2 * M, FH + 2 * M)
    );
}

#[test]
fn a_bad_analysis_area_is_a_usage_error() {
    let tmp = TempDir::new("area-bad");
    write_clean(&tmp.path("f.tif"), [0.2, 0.5, 0.7], [1.6, 1.7, 1.8]);
    for bad in ["0.5,0.5,0.6,0.1", "1,2,3", "a,b,c,d", "0,0,0,1"] {
        for cmd in ["convert", "batch"] {
            let target = if cmd == "convert" {
                s(&tmp.path("f.tif"))
            } else {
                s(&tmp.0)
            };
            let o = negg(&[cmd, &target, "--analysis-area", bad]);
            assert_eq!(o.status.code(), Some(2), "{cmd} {bad}: {}", stderr(&o));
        }
    }
    assert!(!tmp.path("f.negg.json").exists());
}

#[test]
fn batch_flags_held_frames_and_pools_the_good_ones() {
    let tmp = TempDir::new("area-batch");
    let dir = tmp.path("roll");
    for i in 0..3 {
        let d = 0.01 * i as f32;
        write_clean(
            &dir.join(format!("a{i}.tif")),
            [0.2 + d, 0.5 + d, 0.7 + d],
            [1.6, 1.7, 1.8],
        );
    }
    for i in 0..5 {
        let d = 0.01 * i as f32;
        write_held(
            &dir.join(format!("b{i}.tif")),
            [0.2 + d, 0.5 + d, 0.7 + d],
            [1.6, 1.7, 1.8],
        );
    }
    let o = negg(&["batch", &s(&dir), "--out", &s(&tmp.path("out"))]);
    ok(&o);
    let err = stderr(&o);
    for i in 0..5 {
        assert!(
            err.contains(&format!("b{i}.tif: Dmax is at the density cap")),
            "{err}"
        );
    }
    assert!(!err.contains("a0.tif: Dmax"), "{err}");
    let roll = read_json(&dir.join("roll.negg.json"));
    assert_eq!(roll["schema_version"], serde_json::json!(4));
    let frames = roll["pool"]["frames"].as_array().unwrap();
    for f in frames {
        let held = f["source"].as_str().unwrap().starts_with('b');
        assert_eq!(f.get("needs_analysis_area").is_some(), held, "{f}");
        let role = if held { "unused" } else { "inlier" };
        assert_eq!(f["gamma_role"], serde_json::json!(role), "{f}");
    }
    assert!(roll["pool"]["gamma"].is_array());

    // One area fits the held frames' size only, so they are batched again
    // on their own with the area on their film.
    let held_dir = tmp.path("held");
    std::fs::create_dir_all(&held_dir).unwrap();
    for i in 0..5 {
        let name = format!("b{i}.tif");
        std::fs::copy(dir.join(&name), held_dir.join(&name)).unwrap();
    }
    let o = negg(&[
        "batch",
        &s(&held_dir),
        "--analysis-area",
        AREA,
        "--out",
        &s(&tmp.path("out-held")),
    ]);
    ok(&o);
    assert!(!stderr(&o).contains("density cap"), "{}", stderr(&o));
    let roll = read_json(&held_dir.join("roll.negg.json"));
    assert_eq!(
        roll["settings"]["shared"]["geometry"]["analysis_area"]["x"],
        serde_json::json!(0.1)
    );
    for f in roll["pool"]["frames"].as_array().unwrap() {
        assert!(f.get("needs_analysis_area").is_none(), "{f}");
        assert_eq!(f["gamma_role"], serde_json::json!("inlier"), "{f}");
    }
    assert_eq!(
        tiff_size(&tmp.path("out-held/b0.negg.tif")),
        (FW + 2 * M, FH + 2 * M)
    );
    // `negg convert` uses the roll's pool with the same area, and not with
    // another one.
    let frame = s(&held_dir.join("b1.tif"));
    let o = negg(&["convert", &frame, "--out", &s(&tmp.path("one"))]);
    ok(&o);
    assert!(
        stderr(&o).contains("using the roll settings and pool"),
        "{}",
        stderr(&o)
    );
    let o = negg(&[
        "convert",
        &frame,
        "--analysis-area",
        "0.12,0.125,0.76,0.75",
        "--out",
        &s(&tmp.path("one")),
    ]);
    ok(&o);
    assert!(
        stderr(&o).contains("batch the roll again"),
        "{}",
        stderr(&o)
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

#[test]
fn the_session_with_an_area_set_before_pass_1_writes_what_batch_writes() {
    let tmp = TempDir::new("area-parity");
    for d in ["app", "cli"] {
        for i in 0..4 {
            let x = 0.01 * i as f32;
            write_held(
                &tmp.path(&format!("{d}/f{i}.tif")),
                [0.2 + x, 0.5 + x, 0.7 + x],
                [1.6 - x, 1.7, 1.8 + x],
            );
        }
    }
    let app = tmp.path("app");
    let cli = tmp.path("cli");

    // The session: the pre-analysis step (the negative, then the roll's
    // analysis area), then pass 1 and the pool, all in the session.
    let mut session = Session::open_roll(&app).unwrap();
    assert_eq!(
        &session.negative_thumbnail_jpeg(0).unwrap()[..2],
        &[0xFF, 0xD8]
    );
    session.negative_tile(0, 0, 0, 0).unwrap();
    session.set_roll_analysis_area(Some(AREA_RECT)).unwrap();
    session.analyse_all().unwrap();
    assert!(
        session
            .roll_view()
            .frames
            .iter()
            .all(|f| !f.needs_analysis_area)
    );
    // No frame is edited: the roll's analysis area alone makes the flush
    // write every frame sidecar, as `negg batch` writes them (Ruling 139).
    session.flush().unwrap();
    let sidecars = |d: &Path| files(d, |n| n.ends_with(".negg.json"));
    // What the session itself wrote, before any export runs a batch.
    let session_written = sidecars(&app);
    assert_eq!(
        session_written.len(),
        5,
        "four frame sidecars and the roll sidecar: {:?}",
        session_written.keys()
    );
    let views: Vec<_> = (0..4).map(|i| session.frame_view(i).unwrap()).collect();

    // `negg batch --analysis-area` on a separate copy of the same frames.
    ok(&negg(&[
        "batch",
        &s(&cli),
        "--analysis-area",
        AREA,
        "--format",
        "tiff16",
        "--out",
        &s(&tmp.path("out-cli")),
    ]));
    let batch_written = sidecars(&cli);
    assert_eq!(
        session_written.keys().collect::<Vec<_>>(),
        batch_written.keys().collect::<Vec<_>>()
    );
    for (name, bytes) in &session_written {
        assert!(
            *bytes == batch_written[name],
            "{name}: the session's flush differs from negg batch --analysis-area"
        );
    }
    // The estimates and pool records the session shows are the CLI's.
    let roll = negg_io::read_roll_sidecar(&cli.join("roll.negg.json")).unwrap();
    assert_eq!(roll.settings.shared.geometry.analysis_area, Some(AREA_RECT));
    let pool = roll.pool.unwrap();
    for v in &views {
        let stem = v.source.trim_end_matches(".tif");
        let sidecar = negg_io::read_sidecar(&cli.join(format!("{stem}.negg.json"))).unwrap();
        assert_eq!(v.estimates, sidecar.estimates, "frame {}", v.index);
        let record = pool.frames.iter().find(|f| f.source == v.source).cloned();
        assert_eq!(v.pool, record, "frame {}", v.index);
        assert!(!v.needs_analysis_area);
    }

    // The export (the CLI's own flow, Ruling 82) gives the same images.
    let report = session
        .export(
            &ExportRequest {
                scope: ExportScope::Roll,
                format: ExportFormat::Tiff16,
                out_dir: Some(s(&tmp.path("out-app"))),
                web: negg_session::WebOutput::None,
            },
            &mut |_| {},
        )
        .unwrap();
    assert_eq!(
        (report.converted, report.failed),
        (4, 0),
        "{:?}",
        report.errors
    );
    let a = files(&tmp.path("out-app"), |_| true);
    assert_eq!(a.len(), 4);
    assert_eq!(a, files(&tmp.path("out-cli"), |_| true));
}

/// Ruling 139: a roll batched by the CLI, opened in a session, with only
/// the roll's analysis area (or only its crop) changed: the flush rewrites
/// every frame sidecar and `roll.negg.json` exactly as a fresh `negg
/// batch` with the same settings writes them.
#[test]
fn a_roll_edit_alone_brings_every_sidecar_up_to_date() {
    let crop = CropRect {
        x: 0.05,
        y: 0.04,
        width: 0.9,
        height: 0.92,
    };
    for (tag, flag, value) in [
        ("area", "--analysis-area", AREA),
        ("crop", "--crop", "0.05,0.04,0.9,0.92"),
    ] {
        let tmp = TempDir::new(&format!("refresh-{tag}"));
        for d in ["app", "fresh"] {
            for i in 0..4 {
                let x = 0.01 * i as f32;
                write_held(
                    &tmp.path(&format!("{d}/f{i}.tif")),
                    [0.2 + x, 0.5 + x, 0.7 + x],
                    [1.6 - x, 1.7, 1.8 + x],
                );
            }
        }
        let app = tmp.path("app");
        let fresh = tmp.path("fresh");
        ok(&negg(&["batch", &s(&app), "--out", &s(&tmp.path("out-0"))]));
        let sidecars = |d: &Path| files(d, |n| n.ends_with(".negg.json"));
        let before = sidecars(&app);
        assert_eq!(before.len(), 5);

        let mut session = Session::open_roll(&app).unwrap();
        session.analyse_all().unwrap();
        if tag == "area" {
            session.set_roll_analysis_area(Some(AREA_RECT)).unwrap();
        } else {
            let mut roll = session.roll_view().roll;
            roll.shared.geometry.crop = Some(crop);
            session.commit_roll(roll).unwrap();
        }
        session.flush().unwrap();
        let flushed = sidecars(&app);

        ok(&negg(&[
            "batch",
            &s(&fresh),
            flag,
            value,
            "--out",
            &s(&tmp.path("out-1")),
        ]));
        let batch = sidecars(&fresh);
        assert_eq!(
            flushed.keys().collect::<Vec<_>>(),
            batch.keys().collect::<Vec<_>>(),
            "{tag}"
        );
        for (name, bytes) in &flushed {
            assert!(
                *bytes == batch[name],
                "{tag}: {name} differs from a fresh negg batch {flag}"
            );
            assert!(*bytes != before[name], "{tag}: {name} was not rewritten");
        }
        // A second flush writes nothing more.
        session.flush().unwrap();
        assert_eq!(sidecars(&app), flushed);
    }
}

/// Ruling 138: a batch whose analysis area leaves out the clear film edge
/// of a roll pooled on it says so.
#[test]
fn batch_warns_when_the_area_leaves_out_the_film_edge() {
    let tmp = TempDir::new("area-rebate");
    let dir = tmp.path("roll");
    for i in 0..3 {
        let d = 0.01 * i as f32;
        write_clean(
            &dir.join(format!("a{i}.tif")),
            [0.2 + d, 0.5 + d, 0.7 + d],
            [1.6, 1.7, 1.8],
        );
    }
    let o = negg(&["batch", &s(&dir), "--out", &s(&tmp.path("out"))]);
    ok(&o);
    assert!(!stderr(&o).contains("clear film edge"), "{}", stderr(&o));
    let o = negg(&[
        "batch",
        &s(&dir),
        "--analysis-area",
        "0.25,0.25,0.5,0.5",
        "--out",
        &s(&tmp.path("out")),
    ]);
    ok(&o);
    assert!(
        stderr(&o).contains("warning: the analysis area (or crop) leaves out the clear film edge"),
        "{}",
        stderr(&o)
    );
}

/// `negg lock` on a frame whose Dmax is pinned at the density cap locks
/// with a warning (it does not refuse).
#[test]
fn locking_a_pinned_frame_warns() {
    let tmp = TempDir::new("area-lock");
    let dir = tmp.path("roll");
    for i in 0..3 {
        let d = 0.01 * i as f32;
        write_held(
            &dir.join(format!("b{i}.tif")),
            [0.2 + d, 0.5 + d, 0.7 + d],
            [1.6, 1.7, 1.8],
        );
    }
    ok(&negg(&["batch", &s(&dir), "--out", &s(&tmp.path("out"))]));
    let o = negg(&["lock", &s(&dir.join("b0.tif"))]);
    ok(&o);
    assert!(
        stderr(&o).contains("negg lock: warning: b0.tif: Dmax is at the density cap"),
        "{}",
        stderr(&o)
    );
    let side = read_json(&dir.join("b0.negg.json"));
    assert!(side["overrides"]["film"]["gamma"].is_object(), "{side}");
}
