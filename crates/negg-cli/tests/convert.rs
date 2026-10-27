// SPDX-License-Identifier: Apache-2.0

//! End-to-end tests for `negg convert` and `negg batch` through the built
//! binary, on synthetic negatives written as untagged 16-bit TIFFs.

mod common;

use std::fs::File;
use std::path::Path;
use std::process::{Command, Output};

use common::{TempDir, without_icc_date, write_negative_tiff, write_negative_tiff_srgb};
use tiff::decoder::{Decoder, DecodingResult};

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

fn tiff_dimensions(path: &Path) -> (u32, u32) {
    let mut d = Decoder::new(File::open(path).unwrap()).unwrap();
    d.dimensions().unwrap()
}

#[test]
fn convert_writes_image_and_sidecar_and_dumps_stats() {
    let dir = TempDir::new("convert");
    let input = dir.path("roll/frame.tif");
    write_negative_tiff(&input, 64, 48, [0.2, 0.5, 0.7], [1.7, 1.7, 1.6]);
    let out = dir.path("out");
    let o = negg(&[
        "convert",
        input.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--dump-stats",
        "--timing",
    ]);
    let (so, se) = (stdout(&o), stderr(&o));
    assert_eq!(o.status.code(), Some(0), "{so}\n{se}");

    let image = out.join("frame.negg.tif");
    assert!(image.is_file(), "{so}");
    assert_eq!(tiff_dimensions(&image), (64, 48));
    assert!(so.contains("wrote "), "{so}");

    // The sidecar sits beside the source, not in --out.
    let sidecar = dir.path("roll/frame.negg.json");
    assert!(sidecar.is_file(), "{so}");
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&sidecar).unwrap()).unwrap();
    assert_eq!(v["schema_version"], 5);
    assert_eq!(v["source"], "frame.tif");
    assert_eq!(v["settings"]["colour"]["output_space"], "srgb");
    assert!(v["settings_hash"].as_str().unwrap().len() == 64);
    assert!(v["estimates"]["dmin_source"].is_string(), "{v}");
    assert_eq!(v["estimates"]["gamma_source"], "range");
    assert!(v["estimates"]["x0"].is_number());
    assert_eq!(v["negg_version"], env!("CARGO_PKG_VERSION"));

    // The stats block names every estimate and the sizes.
    for key in [
        "dimensions: 64x48",
        "grid cells: 3072",
        "dmin: [",
        "dmax: [",
        "gamma: [",
        "(range)",
        "anchor: ",
        "x0: ",
        "cast: slope_r",
        "matrix: none",
    ] {
        assert!(so.contains(key), "missing {key:?} in\n{so}");
    }
    // The decoder's gamma warning and the timing go to stderr.
    assert!(se.contains("assuming gamma 2.2"), "{se}");
    assert!(se.contains("timing: decode"), "{se}");
    assert!(se.contains("analyse+render"), "{se}");
}

#[test]
fn convert_jpeg_in_another_space_records_the_space_in_the_sidecar() {
    let dir = TempDir::new("jpeg");
    let input = dir.path("frame.tif");
    write_negative_tiff(&input, 40, 30, [0.3; 3], [1.6; 3]);
    let o = negg(&[
        "convert",
        input.to_str().unwrap(),
        "--format",
        "jpeg",
        "--space",
        "adobe_rgb",
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    // Default --out is the input's own directory.
    let jpeg = dir.path("frame.negg.jpg");
    assert!(jpeg.is_file());
    let bytes = std::fs::read(&jpeg).unwrap();
    assert_eq!(&bytes[..2], &[0xFF, 0xD8], "JPEG SOI marker");
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path("frame.negg.json")).unwrap())
            .unwrap();
    assert_eq!(v["settings"]["colour"]["output_space"], "adobe_rgb");
    assert!(v["estimates"]["cast"]["slope_r"].as_f64().is_some());
}

#[test]
fn an_srgb_tagged_neutral_negative_renders_neutral() {
    // ADR 0012: an ICC TIFF is linearised in its own primaries and the
    // step 7 matrix from its colorants maps its neutrals to Rec.2020
    // neutrals, so a neutral negative prints neutral.
    let dir = TempDir::new("icc");
    let input = dir.path("frame.tif");
    write_negative_tiff_srgb(&input, 64, 48, [0.3; 3], [1.6; 3]);
    let o = negg(&["convert", input.to_str().unwrap(), "--dump-stats"]);
    let so = stdout(&o);
    assert_eq!(o.status.code(), Some(0), "{so}\n{}", stderr(&o));
    assert!(so.contains("matrix: camera to Rec.2020 applied"), "{so}");
    let mut d = Decoder::new(File::open(dir.path("frame.negg.tif")).unwrap()).unwrap();
    let DecodingResult::U16(px) = d.read_image().unwrap() else {
        panic!("expected 16-bit output");
    };
    let mut worst = 0.0f32;
    for p in px.as_chunks::<3>().0 {
        let rgb = p.map(|v| f32::from(v) / 65535.0);
        let lab = negg_cms::lab::srgb_to_lab(rgb);
        worst = worst.max(lab[1].hypot(lab[2]));
    }
    assert!(worst < 0.5, "worst C*ab {worst}");
}

#[test]
fn convert_honours_a_settings_file() {
    let dir = TempDir::new("settings");
    let input = dir.path("frame.tif");
    write_negative_tiff(&input, 40, 30, [0.3; 3], [1.6; 3]);
    dir.write(
        "s.json",
        br#"{"film": {"gamma": {"mode": "fixed", "value": [1.5, 1.5, 1.5]}}, "colour": {"auto_colour": false, "output_space": "rec2020"}}"#,
    );
    let o = negg(&[
        "convert",
        input.to_str().unwrap(),
        "--settings",
        dir.path("s.json").to_str().unwrap(),
        "--dump-stats",
    ]);
    let so = stdout(&o);
    assert_eq!(o.status.code(), Some(0), "{so}\n{}", stderr(&o));
    assert!(
        so.contains("gamma: [1.5000, 1.5000, 1.5000] (fixed)"),
        "{so}"
    );
    assert!(so.contains("cells_used 0"), "{so}");
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path("frame.negg.json")).unwrap())
            .unwrap();
    assert_eq!(v["settings"]["colour"]["output_space"], "rec2020");
    assert_eq!(v["estimates"]["gamma_source"], "fixed");
}

#[test]
fn convert_takes_a_sidecar_as_settings() {
    // A sidecar passed as --settings is read through
    // negg_io::migrate and its settings are used.
    let dir = TempDir::new("sidecar-settings");
    let input = dir.path("frame.tif");
    write_negative_tiff(&input, 40, 30, [0.3; 3], [1.6; 3]);
    let o = negg(&["convert", input.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let first = without_icc_date(&dir.path("frame.negg.tif"));
    let mut sidecar: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path("frame.negg.json")).unwrap())
            .unwrap();
    assert_ne!(sidecar["settings"]["print"]["grade"], 60.0);
    sidecar["settings"]["print"]["grade"] = serde_json::json!(60.0);
    dir.write("copy.json", sidecar.to_string().as_bytes());

    let o = negg(&[
        "convert",
        input.to_str().unwrap(),
        "--settings",
        dir.path("copy.json").to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path("frame.negg.json")).unwrap())
            .unwrap();
    assert_eq!(written["settings"]["print"]["grade"], 60.0);
    let second = without_icc_date(&dir.path("frame.negg.tif"));
    assert_ne!(first, second, "grade 60 must change the render");

    // The empty object is the default settings.
    dir.write("empty.json", b"{}");
    let o = negg(&[
        "convert",
        input.to_str().unwrap(),
        "--settings",
        dir.path("empty.json").to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(without_icc_date(&dir.path("frame.negg.tif")), first);

    // JSON with neither Settings keys nor a settings key: usage error.
    dir.write("other.json", br#"{"grade": 60}"#);
    let o = negg(&[
        "convert",
        input.to_str().unwrap(),
        "--settings",
        dir.path("other.json").to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
    assert!(
        stderr(&o).contains("not a settings file or a sidecar"),
        "{}",
        stderr(&o)
    );
}

#[test]
fn convert_errors_use_the_documented_exit_codes() {
    let dir = TempDir::new("errors");
    let input = dir.path("frame.tif");
    write_negative_tiff(&input, 40, 30, [0.3; 3], [1.6; 3]);

    // Invalid settings: usage error.
    dir.write("bad.json", br#"{"print": {"grade": 500}}"#);
    let o = negg(&[
        "convert",
        input.to_str().unwrap(),
        "--settings",
        dir.path("bad.json").to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
    assert!(stderr(&o).contains("grade"), "{}", stderr(&o));

    // Settings file that does not exist, or is not JSON: usage error.
    let o = negg(&[
        "convert",
        input.to_str().unwrap(),
        "--settings",
        dir.path("nope.json").to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(2));
    dir.write("notjson.json", b"{");
    let o = negg(&[
        "convert",
        input.to_str().unwrap(),
        "--settings",
        dir.path("notjson.json").to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(2));

    // Input that does not exist, or does not decode: conversion failure.
    let o = negg(&["convert", dir.path("missing.dng").to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("decode"), "{}", stderr(&o));
    dir.write("junk.tif", b"not a tiff");
    let o = negg(&["convert", dir.path("junk.tif").to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1));
    assert!(!dir.path("junk.negg.json").exists());

    // Unknown flag: clap's usage error.
    let o = negg(&["convert", input.to_str().unwrap(), "--sparkle"]);
    assert_eq!(o.status.code(), Some(2));
}

#[test]
fn batch_converts_every_negative_in_name_order_and_continues_past_failures() {
    let dir = TempDir::new("batch");
    write_negative_tiff(&dir.path("roll/b.tif"), 32, 24, [0.3; 3], [1.6; 3]);
    write_negative_tiff(&dir.path("roll/a.TIFF"), 32, 24, [0.2; 3], [1.7; 3]);
    let source_a = std::fs::read(dir.path("roll/a.TIFF")).unwrap();
    let source_b = std::fs::read(dir.path("roll/b.tif")).unwrap();
    dir.write("roll/c.dng", b"not a real raw");
    dir.write("roll/notes.txt", b"ignored");
    // A previous output is skipped by its `.negg.` infix, never decoded.
    // (An unreadable sidecar beside a frame now fails that frame instead of
    // being replaced; tests/roll.rs covers it.)
    dir.write("roll/old.negg.tif", b"not a tiff");
    std::fs::create_dir_all(dir.path("roll/sub")).unwrap();
    let out = dir.path("positives");
    let o = negg(&[
        "batch",
        dir.path("roll").to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--dump-stats",
    ]);
    let (so, se) = (stdout(&o), stderr(&o));
    // One frame failed, so the exit status is 1, but the others were
    // converted and the summary is printed last.
    assert_eq!(o.status.code(), Some(1), "{so}\n{se}");
    assert!(out.join("a.negg.tif").is_file(), "{so}");
    assert!(out.join("b.negg.tif").is_file(), "{so}");
    assert!(!out.join("c.negg.tif").exists());
    assert!(dir.path("roll/a.negg.json").is_file());
    assert!(dir.path("roll/b.negg.json").is_file());
    let lines: Vec<&str> = so.lines().collect();
    let a = lines
        .iter()
        .position(|l| l.starts_with("wrote ") && l.contains("a.negg.tif"));
    let b = lines
        .iter()
        .position(|l| l.starts_with("wrote ") && l.contains("b.negg.tif"));
    assert!(a.unwrap() < b.unwrap(), "{so}");
    assert_eq!(
        *lines.last().unwrap(),
        "converted 2 of 3 frames (1 failed)",
        "{so}"
    );
    assert_eq!(so.matches("dmin: [").count(), 2, "{so}");
    assert!(se.contains("c.dng"), "{se}");

    // Everything converted: exit 0.
    std::fs::remove_file(dir.path("roll/c.dng")).unwrap();
    let o = negg(&["batch", dir.path("roll").to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert!(stdout(&o).ends_with("converted 2 of 2 frames (0 failed)\n"));
    // Default --out is the directory itself, and the sources survive.
    assert!(dir.path("roll/a.negg.tif").is_file());
    assert!(dir.path("roll/b.negg.tif").is_file());
    assert_eq!(std::fs::read(dir.path("roll/a.TIFF")).unwrap(), source_a);
    assert_eq!(std::fs::read(dir.path("roll/b.tif")).unwrap(), source_b);

    // A second run skips the previous outputs (`*.negg.tif`) and the
    // sidecars, so it finds the same two frames and still leaves the
    // sources alone.
    let o = negg(&["batch", dir.path("roll").to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert!(stdout(&o).ends_with("converted 2 of 2 frames (0 failed)\n"));
    assert_eq!(std::fs::read(dir.path("roll/a.TIFF")).unwrap(), source_a);
    assert_eq!(std::fs::read(dir.path("roll/b.tif")).unwrap(), source_b);

    // Nothing to convert is not an error.
    std::fs::create_dir_all(dir.path("empty")).unwrap();
    let o = negg(&["batch", dir.path("empty").to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(stdout(&o), "converted 0 of 0 frames (0 failed)\n");

    // Not a directory: usage error.
    let o = negg(&["batch", dir.path("roll/b.tif").to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(2));
    assert!(stderr(&o).contains("not a directory"));
    let o = negg(&["batch", dir.path("missing").to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(2));
}

#[test]
fn convert_never_overwrites_a_tiff_source_with_the_default_out() {
    let dir = TempDir::new("no-overwrite");
    let input = dir.path("frame.tif");
    write_negative_tiff(&input, 40, 30, [0.3; 3], [1.6; 3]);
    let source = std::fs::read(&input).unwrap();
    let o = negg(&["convert", input.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert!(dir.path("frame.negg.tif").is_file());
    assert!(stdout(&o).contains("frame.negg.tif"), "{}", stdout(&o));
    assert_eq!(std::fs::read(&input).unwrap(), source);
}

#[test]
fn batch_fails_a_second_source_with_the_same_output_path() {
    let dir = TempDir::new("duplicate");
    // `a.tif` and `a.tiff` both resolve to `a.negg.tif`; `a.tif` sorts
    // first and wins.
    write_negative_tiff(&dir.path("roll/a.tif"), 32, 24, [0.2; 3], [1.7; 3]);
    write_negative_tiff(&dir.path("roll/a.tiff"), 48, 36, [0.3; 3], [1.6; 3]);
    let out = dir.path("positives");
    let o = negg(&[
        "batch",
        dir.path("roll").to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    let (so, se) = (stdout(&o), stderr(&o));
    assert_eq!(o.status.code(), Some(1), "{so}\n{se}");
    assert!(so.ends_with("converted 1 of 2 frames (1 failed)\n"), "{so}");
    assert!(se.contains("a.tiff"), "{se}");
    assert!(se.contains("output path already produced by"), "{se}");
    // The output is the first source's, not overwritten by the second.
    assert_eq!(tiff_dimensions(&out.join("a.negg.tif")), (32, 24));
}
