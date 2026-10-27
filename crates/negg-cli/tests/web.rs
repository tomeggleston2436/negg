// SPDX-License-Identifier: Apache-2.0

//! The web output (ADR 0035): `--web` writes `<out>/web/<stem>.negg.jpg`
//! beside the normal output, `--web-only` writes only that. The web copy
//! is an sRGB JPEG with the profile embedded whatever `--space` says, with
//! a long edge of 2048 px and never upscaled.

mod common;

use std::path::Path;
use std::process::{Command, Output};

use common::{TempDir, write_negative_tiff};
use image::ImageDecoder;
use image::codecs::jpeg::JpegDecoder;
use negg_types::settings::OutputSpace;

fn negg(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_negg"))
        .args(args)
        .output()
        .expect("run negg")
}

fn ok(args: &[&str]) -> String {
    let o = negg(args);
    assert_eq!(
        o.status.code(),
        Some(0),
        "negg {args:?}\n{}\n{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn arg(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// The size and embedded ICC profile of a JPEG.
fn jpeg_info(path: &Path) -> ((u32, u32), Option<Vec<u8>>) {
    let file = std::io::BufReader::new(std::fs::File::open(path).unwrap());
    let mut d = JpegDecoder::new(file).unwrap();
    let dims = d.dimensions();
    (dims, d.icc_profile().unwrap())
}

/// The names of the files (not directories) in `dir`, sorted.
fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_file())
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

const DMIN: [f32; 3] = [0.25, 0.55, 0.75];
const GAMMA: [f32; 3] = [1.6, 1.7, 1.8];

#[test]
fn web_also_writes_both_and_web_is_srgb_2048() {
    let dir = TempDir::new("web-also");
    let input = dir.path("roll/big.tif");
    write_negative_tiff(&input, 2400, 1600, DMIN, GAMMA);
    let (plain, web) = (dir.path("plain"), dir.path("web-out"));
    let base = [
        "convert",
        &arg(&input),
        "--no-roll",
        "--format",
        "tiff16",
        "--space",
        "adobe_rgb",
    ];
    let mut a: Vec<&str> = base.to_vec();
    let plain_arg = arg(&plain);
    a.extend(["--out", &plain_arg]);
    ok(&a);
    let mut b: Vec<&str> = base.to_vec();
    let web_arg = arg(&web);
    b.extend(["--out", &web_arg, "--web"]);
    let stdout = ok(&b);

    // The original is exactly what a run without --web writes.
    let original = std::fs::read(web.join("big.negg.tif")).unwrap();
    assert!(original == std::fs::read(plain.join("big.negg.tif")).unwrap());
    // The web copy: sRGB, long edge 2048, under web/.
    let web_jpg = web.join("web").join("big.negg.jpg");
    assert!(stdout.contains(&arg(&web_jpg)), "{stdout}");
    let ((w, h), icc) = jpeg_info(&web_jpg);
    assert_eq!((w, h), (2048, 1365));
    assert_eq!(
        icc.as_deref(),
        Some(
            negg_cms::profile_bytes(OutputSpace::Srgb)
                .unwrap()
                .as_slice()
        )
    );
    assert_eq!(names(&web), ["big.negg.tif"]);
    assert_eq!(names(&web.join("web")), ["big.negg.jpg"]);
    // The sidecar records the frame's own output space.
    let v = common::read_json(&dir.path("roll/big.negg.json"));
    assert_eq!(v["settings"]["colour"]["output_space"], "adobe_rgb");
}

#[test]
fn web_only_writes_no_original() {
    let dir = TempDir::new("web-only");
    let input = dir.path("roll/f.tif");
    write_negative_tiff(&input, 300, 200, DMIN, GAMMA);
    let out = dir.path("out");
    let stdout = ok(&[
        "convert",
        &arg(&input),
        "--format",
        "jpeg",
        "--web-only",
        "--out",
        &arg(&out),
    ]);
    assert!(names(&out).is_empty(), "{:?}", names(&out));
    let web_jpg = out.join("web").join("f.negg.jpg");
    assert_eq!(names(&out.join("web")), ["f.negg.jpg"]);
    assert!(stdout.contains(&arg(&web_jpg)), "{stdout}");
    // Never upscaled.
    assert_eq!(jpeg_info(&web_jpg).0, (300, 200));
    // The sidecar is written as for any export.
    assert!(dir.path("roll/f.negg.json").is_file());
}

#[test]
fn web_export_of_a_small_crop_keeps_its_size() {
    let dir = TempDir::new("web-crop");
    let input = dir.path("roll/c.tif");
    write_negative_tiff(&input, 1200, 800, DMIN, GAMMA);
    let out = dir.path("out");
    ok(&[
        "convert",
        &arg(&input),
        "--crop",
        "0,0,0.75,1",
        "--web-only",
        "--out",
        &arg(&out),
    ]);
    assert_eq!(jpeg_info(&out.join("web").join("c.negg.jpg")).0, (900, 800));
}

#[test]
fn batch_web_skips_the_web_folder_next_time() {
    let dir = TempDir::new("web-batch");
    let roll = dir.path("roll");
    write_negative_tiff(&roll.join("a.tif"), 240, 160, DMIN, GAMMA);
    write_negative_tiff(&roll.join("b.tif"), 240, 160, [0.26, 0.56, 0.76], GAMMA);
    for _ in 0..2 {
        let stdout = ok(&["batch", &arg(&roll), "--format", "jpeg", "--web"]);
        assert!(
            stdout.contains("converted 2 of 2 frames (0 failed)"),
            "{stdout}"
        );
    }
    assert_eq!(names(&roll.join("web")), ["a.negg.jpg", "b.negg.jpg"]);
    assert!(roll.join("a.negg.jpg").is_file() && roll.join("b.negg.jpg").is_file());
}

#[test]
fn web_and_web_only_conflict() {
    let dir = TempDir::new("web-conflict");
    let input = dir.path("f.tif");
    write_negative_tiff(&input, 64, 48, DMIN, GAMMA);
    let o = negg(&["convert", &arg(&input), "--web", "--web-only"]);
    assert_eq!(o.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(stderr.contains("cannot be used with"), "{stderr}");
    assert!(!dir.path("web").exists() && !dir.path("f.negg.json").exists());
}
