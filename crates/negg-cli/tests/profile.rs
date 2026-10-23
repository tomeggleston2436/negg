// SPDX-License-Identifier: Apache-2.0

//! End-to-end tests for `negg profile rig` and `negg profile film` on
//! synthetic linear (32-bit float TIFF) frames, and for using the profiles
//! they write with `--rig` and `--film`.

mod common;

use std::process::{Command, Output};

use common::{TempDir, negative_linear, read_json, write_float_tiff, write_negative_tiff};
use negg_types::profile::{DensityModelKind, FilmMethod, RigMethod};

fn negg(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_negg"))
        .args(args)
        .output()
        .expect("run negg")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

const W: usize = 192;
const H: usize = 128;

/// The synthetic rig's crosstalk: `M[c][j]` is channel `c`'s response to
/// band `j`.
const M: [[f32; 3]; 3] = [[1.0, 0.08, 0.02], [0.1, 1.0, 0.12], [0.03, 0.07, 1.0]];

/// Mild cos^4 vignetting.
fn vignette(x: usize, y: usize) -> f32 {
    let dx = x as f32 + 0.5 - W as f32 / 2.0;
    let dy = y as f32 + 0.5 - H as f32 / 2.0;
    let theta = ((dx * dx + dy * dy).sqrt() / 300.0).atan();
    theta.cos().powi(4)
}

/// A frame of the synthetic rig: the scene's per-band transmitted light
/// `scene(x, y)` mixed by `M` and vignetted.
fn rig_frame(scene: impl Fn(usize, usize) -> [f32; 3]) -> Vec<f32> {
    let mut data = Vec::with_capacity(W * H * 3);
    for y in 0..H {
        for x in 0..W {
            let s = scene(x, y);
            let v = vignette(x, y);
            for row in &M {
                data.push(v * (row[0] * s[0] + row[1] * s[1] + row[2] * s[2]));
            }
        }
    }
    data
}

/// The 12 wedge patches: a 4 by 3 layout, patch `k` at density
/// `0.1 + 0.25 k`. Returns `(x0, y0, x1, y1, density)` in pixels.
fn wedge_patches() -> Vec<(usize, usize, usize, usize, f32)> {
    let (cw, ch) = (W / 4, H / 3);
    (0..12)
        .map(|k| {
            let (i, j) = (k % 4, k / 4);
            (
                i * cw + 4,
                j * ch + 4,
                (i + 1) * cw - 4,
                (j + 1) * ch - 4,
                0.1 + 0.25 * k as f32,
            )
        })
        .collect()
}

fn write_rig_inputs(dir: &TempDir) {
    let light = 0.6;
    write_float_tiff(
        &dir.path("rig/flat.tif"),
        W,
        H,
        &rig_frame(|_, _| [light; 3]),
    );
    for (j, name) in ["band_r", "band_g", "band_b"].iter().enumerate() {
        let frame = rig_frame(|_, _| {
            let mut s = [0.0; 3];
            s[j] = 0.7;
            s
        });
        write_float_tiff(&dir.path(&format!("rig/{name}.tif")), W, H, &frame);
    }
    let patches = wedge_patches();
    let wedge = rig_frame(|x, y| {
        let d = patches
            .iter()
            .find(|p| x >= p.0 && x < p.2 && y >= p.1 && y < p.3)
            .map_or(0.0, |p| p.4);
        [light * 10f32.powf(-d); 3]
    });
    write_float_tiff(&dir.path("wedge/wedge.tif"), W, H, &wedge);
    let mut csv = String::from("# synthetic 12-step wedge\npatch,x,y,width,height,density\n");
    for (k, p) in patches.iter().enumerate() {
        csv.push_str(&format!(
            "s{k},{},{},{},{},{}\n",
            p.0 as f32 / W as f32,
            p.1 as f32 / H as f32,
            (p.2 - p.0) as f32 / W as f32,
            (p.3 - p.1) as f32 / H as f32,
            p.4
        ));
    }
    dir.write("wedge/wedge.csv", csv.as_bytes());
}

/// Asserts that every float literal in `text` has at most 6 significant
/// digits (the CLI rounds measured values before writing).
fn assert_short_floats(text: &str) {
    for token in text.split(|c: char| !(c.is_ascii_digit() || ".eE+-".contains(c))) {
        if !token.contains('.') || token.parse::<f64>().is_err() {
            continue;
        }
        let mantissa = token.split(['e', 'E']).next().unwrap();
        let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
        let significant = digits.trim_start_matches('0').trim_end_matches('0');
        assert!(significant.len() <= 6, "{token} in\n{text}");
    }
}

#[test]
fn profile_rig_measures_flat_crosstalk_and_wedge() {
    let dir = TempDir::new("profile-rig");
    write_rig_inputs(&dir);
    let out = dir.path("rig/bench.rig.negg.toml");
    let r = |n: &str| {
        dir.path(&format!("rig/{n}.tif"))
            .to_str()
            .unwrap()
            .to_owned()
    };
    let o = negg(&[
        "profile",
        "rig",
        "--name",
        "bench",
        "--source",
        "synthetic test rig",
        "--out",
        out.to_str().unwrap(),
        "--measured-on",
        "2026-01-02",
        "--flat",
        &r("flat"),
        "--band-r",
        &r("band_r"),
        "--band-g",
        &r("band_g"),
        "--band-b",
        &r("band_b"),
        "--wedge",
        dir.path("wedge/wedge.tif").to_str().unwrap(),
        "--densities",
        dir.path("wedge/wedge.csv").to_str().unwrap(),
    ]);
    let (so, se) = (stdout(&o), stderr(&o));
    assert_eq!(o.status.code(), Some(0), "{so}\n{se}");
    assert!(so.contains("flat_field, crosstalk, density_fit"), "{so}");
    // A neutral wedge cannot identify the cross terms.
    assert!(se.contains("diagonal model"), "{se}");

    let p = negg_io::read_rig_profile(&out).unwrap();
    p.validate().unwrap();
    assert_eq!(p.name, "bench");
    assert_eq!(p.measured_on, "2026-01-02");
    assert_eq!(
        p.method,
        [
            RigMethod::FlatField,
            RigMethod::Crosstalk,
            RigMethod::DensityFit
        ]
    );
    let flat = p.flat_field.as_ref().unwrap();
    assert_eq!(flat.frame, "flat.tif");
    assert_eq!((flat.width, flat.height), (W as u32, H as u32));
    let ct = p.crosstalk.as_ref().unwrap();
    assert_eq!(ct.frame_r, "band_r.tif");
    for (got, want) in ct.m.iter().flatten().zip(M.iter().flatten()) {
        assert!((got - want).abs() < 2e-3, "{:?}", ct.m);
    }
    let fit = p.density_fit.as_ref().unwrap();
    assert_eq!(fit.model, DensityModelKind::Diagonal);
    assert_eq!(fit.n_patches, 12);
    for c in 0..3 {
        assert!((fit.b[c] + 1.0).abs() < 0.01, "{:?}", fit.b);
        assert_eq!(fit.c[c], [0.0; 3]);
    }
    assert!(fit.residual_rms < 0.01, "{}", fit.residual_rms);
    assert_eq!(p.residual_rms, fit.residual_rms);
    // Every float written has at most 6 significant digits.
    let text = std::fs::read_to_string(&out).unwrap();
    assert!(!text.contains("TODO"), "{text}");
    assert_short_floats(&text);

    // The profile loads as a runtime correction (the flat is decoded from
    // the path stored relative to the profile) and renders a frame.
    let rig = negg_cli::roll::load_rig(&out).unwrap();
    assert!(rig.flat.is_some() && rig.unmix.is_some() && rig.density.is_some());
    let frame = dir.path("roll/a.tif");
    write_float_tiff(
        &frame,
        W,
        H,
        &negative_linear(W, H, [0.2, 0.5, 0.7], [1.6, 1.7, 1.8], |_, _| None),
    );
    let o = negg(&[
        "convert",
        frame.to_str().unwrap(),
        "--rig",
        out.to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let s = read_json(&dir.path("roll/a.negg.json"));
    assert_eq!(
        s["settings"]["profiles"]["rig"].as_str().unwrap(),
        out.to_str().unwrap()
    );
}

#[test]
fn profile_rig_rejects_bad_command_lines_and_frames() {
    let dir = TempDir::new("profile-rig-usage");
    write_rig_inputs(&dir);
    let out = dir.path("rig/x.rig.negg.toml");
    let base = |extra: &[&str]| {
        let mut a = vec![
            "profile".to_owned(),
            "rig".into(),
            "--name".into(),
            "x".into(),
            "--source".into(),
            "test".into(),
            "--out".into(),
            out.to_str().unwrap().into(),
        ];
        a.extend(extra.iter().map(|s| s.to_string()));
        a
    };
    let run = |extra: &[&str]| {
        let a = base(extra);
        negg(&a.iter().map(String::as_str).collect::<Vec<_>>())
    };
    let flat = dir.path("rig/flat.tif");
    let flat = flat.to_str().unwrap();
    // Nothing to measure; bands not all three; wedge without densities.
    for extra in [
        vec![],
        vec!["--band-r", flat],
        vec!["--wedge", flat],
        vec!["--densities", flat],
    ] {
        let o = run(&extra);
        assert_eq!(o.status.code(), Some(2), "{extra:?}: {}", stderr(&o));
    }
    // A flat outside the profile's directory.
    let o = run(&["--flat", dir.path("wedge/wedge.tif").to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
    assert!(stderr(&o).contains("must lie under"), "{}", stderr(&o));
    // An integer TIFF is not linear.
    let int = dir.path("rig/int.tif");
    write_negative_tiff(&int, W, H, [0.1; 3], [0.1; 3]);
    let o = run(&["--flat", int.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    assert!(stderr(&o).contains("must be linear"), "{}", stderr(&o));
    // A placeholder source is never written.
    let o = negg(&[
        "profile",
        "rig",
        "--name",
        "x",
        "--source",
        "TODO(measure): later",
        "--out",
        out.to_str().unwrap(),
        "--flat",
        flat,
    ]);
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
    assert!(!out.exists());
    // The flat on its own works and dates default to today.
    let o = run(&["--flat", flat]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let p = negg_io::read_rig_profile(&out).unwrap();
    assert_eq!(p.measured_on, negg_cli::profile::today_utc());
    assert_eq!(p.residual_rms, 0.0);
    assert!(p.density_fit.is_none() && p.crosstalk.is_none());
}

/// Ratios and offsets of the synthetic stock's grey axis: a grey patch
/// with green density `d_G` above base has `d_c = ratio_c * d_G + o_c`.
const RATIO: [f32; 3] = [0.92, 1.0, 1.12];
const OFFSET: [f32; 3] = [0.02, 0.0, -0.03];
const DMIN: [f32; 3] = [0.25, 0.55, 0.75];
const FW: usize = 160;
const FH: usize = 120;

/// Grey patch squares of the card frame, `(x0, y0, x1, y1, d_G)`.
const CARD: [(usize, usize, usize, usize, f32); 3] = [
    (30, 40, 55, 80, 0.3),
    (68, 40, 93, 80, 0.7),
    (106, 40, 131, 80, 1.1),
];

fn write_film_roll(dir: &TempDir) {
    for (i, name) in ["f1.tif", "f2.tif", "f3.tif"].iter().enumerate() {
        let gamma = [1.6 + 0.01 * i as f32, 1.7, 1.85];
        let data = negative_linear(FW, FH, DMIN, gamma, |_, _| None);
        write_float_tiff(&dir.path(&format!("roll/{name}")), FW, FH, &data);
    }
    let card = negative_linear(FW, FH, DMIN, [1.6, 1.7, 1.85], |x, y| {
        CARD.iter()
            .find(|p| x >= p.0 && x < p.2 && y >= p.1 && y < p.3)
            .map(|p| [0, 1, 2].map(|c| RATIO[c] * p.4 + OFFSET[c]))
    });
    write_float_tiff(&dir.path("roll/card.tif"), FW, FH, &card);
    let mut csv = String::from("file,x,y,width,height\n");
    for p in CARD {
        csv.push_str(&format!(
            "card.tif,{},{},{},{}\n",
            p.0 as f32 / FW as f32,
            p.1 as f32 / FH as f32,
            (p.2 - p.0) as f32 / FW as f32,
            (p.3 - p.1) as f32 / FH as f32
        ));
    }
    dir.write("patches.csv", csv.as_bytes());
}

#[test]
fn profile_film_recovers_the_synthetic_ratios_and_renders_with_them() {
    let dir = TempDir::new("profile-film");
    write_film_roll(&dir);
    let out = dir.path("stock.film.negg.toml");
    let o = negg(&[
        "profile",
        "film",
        dir.path("roll").to_str().unwrap(),
        "--patches",
        dir.path("patches.csv").to_str().unwrap(),
        "--name",
        "synthetic stock",
        "--source",
        "synthetic roll with a grey card",
        "--stock",
        "Synthetic 400",
        "--iso",
        "400",
        "--out",
        out.to_str().unwrap(),
        "--measured-on",
        "2026-03-04",
    ]);
    let (so, se) = (stdout(&o), stderr(&o));
    assert_eq!(o.status.code(), Some(0), "{so}\n{se}");
    let p = negg_io::read_film_profile(&out).unwrap();
    p.validate().unwrap();
    assert_eq!(p.method, FilmMethod::GreyCardRoll);
    assert_eq!(p.rig, None);
    assert_eq!(p.measured_on, "2026-03-04");
    assert_eq!(p.stock.iso, 400);
    assert_eq!(p.stock.process, "C-41");
    let g = p.gamma.as_ref().unwrap();
    assert_eq!(g.gamma_source, "neutral_fit");
    assert_eq!(g.n_patches, 3);
    assert_eq!(g.ratio[1], 1.0);
    assert_eq!(g.offset[1], 0.0);
    for c in [0, 2] {
        assert!((g.ratio[c] - RATIO[c]).abs() < 0.01, "{:?}", g.ratio);
        let want = -OFFSET[c] / RATIO[c];
        assert!((g.offset[c] - want).abs() < 0.01, "{:?}", g.offset);
    }
    let base = p.base.as_ref().unwrap();
    for (got, want) in base.dmin.iter().zip(DMIN) {
        assert!((got - want).abs() < 1e-3, "{:?}", base.dmin);
    }
    assert!(p.residual_rms < 0.01, "{}", p.residual_rms);
    assert_short_floats(&std::fs::read_to_string(&out).unwrap());

    // Rendering the roll with the profile records the film correction.
    let o = negg(&[
        "batch",
        dir.path("roll").to_str().unwrap(),
        "--film",
        out.to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let s = read_json(&dir.path("roll/card.negg.json"));
    assert_eq!(s["estimates"]["gamma_source"], "profile");
    assert_eq!(s["estimates"]["cast_source"], "identity");
    assert_ne!(s["estimates"]["neutral_offset"][0], 0.0);
    assert_eq!(
        s["settings"]["profiles"]["film"].as_str().unwrap(),
        out.to_str().unwrap()
    );
    let rs = read_json(&dir.path("roll/roll.negg.json"));
    assert!(rs["pool"]["cast"].is_null(), "{rs}");

    // A patch naming a file that is not in the roll is an error.
    dir.write(
        "bad.csv",
        b"file,x,y,width,height\nnope.tif,0.2,0.2,0.2,0.2\n",
    );
    let o = negg(&[
        "profile",
        "film",
        dir.path("roll").to_str().unwrap(),
        "--patches",
        dir.path("bad.csv").to_str().unwrap(),
        "--name",
        "n",
        "--source",
        "s",
        "--stock",
        "S",
        "--iso",
        "100",
        "--out",
        dir.path("bad.film.negg.toml").to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    assert!(stderr(&o).contains("nope.tif"), "{}", stderr(&o));

    // One grey patch: the film base is added as a second point (Ruling 58),
    // and n_patches counts both.
    let p = CARD[1];
    dir.write(
        "one.csv",
        format!(
            "file,x,y,width,height\ncard.tif,{},{},{},{}\n",
            p.0 as f32 / FW as f32,
            p.1 as f32 / FH as f32,
            (p.2 - p.0) as f32 / FW as f32,
            (p.3 - p.1) as f32 / FH as f32
        )
        .as_bytes(),
    );
    let one = dir.path("one.film.negg.toml");
    let o = negg(&[
        "profile",
        "film",
        dir.path("roll").to_str().unwrap(),
        "--patches",
        dir.path("one.csv").to_str().unwrap(),
        "--name",
        "n",
        "--source",
        "s",
        "--stock",
        "S",
        "--iso",
        "100",
        "--out",
        one.to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let p = negg_io::read_film_profile(&one).unwrap();
    assert_eq!(p.gamma.unwrap().n_patches, 2);
    assert_eq!(p.measured_on, negg_cli::profile::today_utc());
}
