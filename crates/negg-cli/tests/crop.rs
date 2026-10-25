// SPDX-License-Identifier: Apache-2.0

//! End-to-end tests for the crop (Rulings 70, 90, 91, 92): the crop is the
//! area the analysis sees, the export is cropped, rotated and flipped, and
//! `--crop` sets the roll's shared crop. Synthetic data only.

mod common;

use std::fs::File;
use std::path::Path;
use std::process::{Command, Output};

use common::{TempDir, negative_linear, read_json, write_float_tiff, write_negative_tiff};
use negg_types::{CropRect, Geometry};
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

/// Width, height and 16-bit RGB samples of a TIFF.
fn read_tiff16(path: &Path) -> (usize, usize, Vec<u16>) {
    let mut d = Decoder::new(File::open(path).unwrap()).unwrap();
    let (w, h) = d.dimensions().unwrap();
    match d.read_image().unwrap() {
        DecodingResult::U16(v) => (w as usize, h as usize, v),
        _ => panic!("expected 16-bit samples"),
    }
}

fn crop_geometry(x: f32, y: f32, width: f32, height: f32) -> Geometry {
    Geometry {
        crop: Some(CropRect {
            x,
            y,
            width,
            height,
        }),
        ..Geometry::default()
    }
}

/// The value of `key` (a `[r, g, b]` line of `--dump-stats`) as numbers.
fn stat3(stats: &str, key: &str) -> [f64; 3] {
    let line = stats
        .lines()
        .find(|l| l.starts_with(&format!("{key}: [")))
        .unwrap_or_else(|| panic!("no {key} in {stats}"));
    let inner = &line[line.find('[').unwrap() + 1..line.find(']').unwrap()];
    let v: Vec<f64> = inner
        .split(',')
        .map(|s| s.trim().parse().unwrap())
        .collect();
    [v[0], v[1], v[2]]
}

#[test]
fn a_crop_to_the_film_removes_the_holder_from_the_analysis() {
    let dir = TempDir::new("crop-surround");
    let (aw, ah) = (200, 150);
    let a = negative_linear(aw, ah, [0.2, 0.5, 0.7], [1.7, 1.7, 1.6], |_, _| None);
    write_float_tiff(&dir.path("a/a.tif"), aw, ah, &a);
    // B: A placed at (50, 30) on a 300 x 210 holder of linear 1e-6.
    let (bw, bh, ox, oy) = (300, 210, 50, 30);
    let mut b = vec![1e-6f32; bw * bh * 3];
    for y in 0..ah {
        for x in 0..aw {
            for c in 0..3 {
                b[((oy + y) * bw + ox + x) * 3 + c] = a[(y * aw + x) * 3 + c];
            }
        }
    }
    write_float_tiff(&dir.path("b/b.tif"), bw, bh, &b);

    let stats = |input: &Path, extra: &[&str]| -> String {
        let mut args = vec![
            "convert",
            input.to_str().unwrap(),
            "--no-roll",
            "--dump-stats",
        ];
        args.extend_from_slice(extra);
        let o = negg(&args);
        assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
        stdout(&o)
    };
    let own = stats(&dir.path("a/a.tif"), &[]);
    // Without a crop the holder pins Dmax at the 6.0 cap.
    let wide = stats(&dir.path("b/b.tif"), &[]);
    for d in stat3(&wide, "dmax") {
        assert!((d - 6.0).abs() < 1e-3, "{wide}");
    }
    let crop = format!(
        "{},{},{},{}",
        ox as f64 / bw as f64,
        oy as f64 / bh as f64,
        aw as f64 / bw as f64,
        ah as f64 / bh as f64
    );
    let g = crop_geometry(
        ox as f32 / bw as f32,
        oy as f32 / bh as f32,
        aw as f32 / bw as f32,
        ah as f32 / bh as f32,
    );
    assert_eq!(g.crop_pixels(bw, bh), (ox, oy, ox + aw, oy + ah));
    // No buffer: within 0.01 of A; the default buffer: within 0.02.
    std::fs::write(
        dir.path("nobuffer.json"),
        r#"{"geometry": {"analysis_buffer": 0.0}}"#,
    )
    .unwrap();
    let settings = dir.path("nobuffer.json");
    let cases: [(Vec<&str>, f64); 2] = [
        (
            vec!["--crop", &crop, "--settings", settings.to_str().unwrap()],
            0.01,
        ),
        (vec!["--crop", &crop], 0.02),
    ];
    for (extra, tol) in cases {
        let cropped = stats(&dir.path("b/b.tif"), &extra);
        assert!(
            cropped.contains(&format!("dimensions: {aw}x{ah}")),
            "{cropped}"
        );
        for key in ["dmin", "dmax", "gamma"] {
            let (want, got) = (stat3(&own, key), stat3(&cropped, key));
            for c in 0..3 {
                assert!((want[c] - got[c]).abs() < tol, "{key}: {own}\n{cropped}");
            }
        }
    }
    let s = read_json(&dir.path("b/b.negg.json"));
    assert_eq!(s["schema_version"], 4);
    assert!(s["settings"]["geometry"]["crop"].is_object(), "{s}");
}

#[test]
fn the_border_source_follows_the_crop() {
    let dir = TempDir::new("crop-border");
    let input = dir.path("f/f.tif");
    // 160 x 120 with a 15 px rebate of film base on every side.
    write_negative_tiff(&input, 160, 120, [0.2, 0.5, 0.7], [1.7, 1.7, 1.6]);
    let run = |crop: &str| {
        let o = negg(&[
            "convert",
            input.to_str().unwrap(),
            "--no-roll",
            "--crop",
            crop,
        ]);
        assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
        read_json(&dir.path("f/f.negg.json"))["estimates"]["dmin_source"].clone()
    };
    // Keeps the rebate: border.
    assert_eq!(run("0,0,1,1"), "border");
    // Inside the image area: percentile.
    assert_eq!(run("0.2,0.2,0.6,0.6"), "percentile");
}

#[test]
fn a_crop_under_64_analysis_cells_fails_the_frame() {
    let dir = TempDir::new("crop-small");
    let input = dir.path("f/f.tif");
    write_negative_tiff(&input, 64, 48, [0.2, 0.5, 0.7], [1.7, 1.7, 1.6]);
    let o = negg(&[
        "convert",
        input.to_str().unwrap(),
        "--no-roll",
        "--crop",
        "0.4,0.4,0.1,0.1",
    ]);
    assert_eq!(o.status.code(), Some(1), "{}", stdout(&o));
    assert!(
        stderr(&o).contains("crop too small to analyse"),
        "{}",
        stderr(&o)
    );
    assert!(!dir.path("f/f.negg.tif").exists());
}

#[test]
fn export_is_cropped_then_rotated_then_flipped() {
    let dir = TempDir::new("crop-export");
    let input = dir.path("f/f.tif");
    write_negative_tiff(&input, 96, 64, [0.2, 0.5, 0.7], [1.7, 1.7, 1.6]);
    let crop = "0.1,0.125,0.75,0.625";
    let g = crop_geometry(0.1, 0.125, 0.75, 0.625);
    let (cw, ch) = g.output_size(96, 64);

    // Crop only.
    let o = negg(&[
        "convert",
        input.to_str().unwrap(),
        "--no-roll",
        "--crop",
        crop,
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let (w, h, plain) = read_tiff16(&dir.path("f/f.negg.tif"));
    assert_eq!((w, h), (cw, ch));

    // Crop, rotate 90 and flip: the same analysis, the pixels moved.
    std::fs::write(
        dir.path("turn.json"),
        r#"{"geometry": {"rotate_degrees": 90, "flip_horizontal": true}}"#,
    )
    .unwrap();
    let o = negg(&[
        "convert",
        input.to_str().unwrap(),
        "--no-roll",
        "--settings",
        dir.path("turn.json").to_str().unwrap(),
        "--crop",
        crop,
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let (tw, th, turned) = read_tiff16(&dir.path("f/f.negg.tif"));
    assert_eq!((tw, th), (ch, cw));
    let mut full = g;
    full.rotate_degrees = 90;
    full.flip_horizontal = true;
    assert_eq!(full.output_size(96, 64), (tw, th));
    // Rotating 90 clockwise then mirroring is a transpose: output (x, y) is
    // the cropped pixel (y, x).
    for y in 0..th {
        for x in 0..tw {
            let o = (y * tw + x) * 3;
            let p = (x * w + y) * 3;
            assert_eq!(turned[o..o + 3], plain[p..p + 3], "({x}, {y})");
        }
    }
}

#[test]
fn batch_crop_is_stored_in_the_roll_and_a_frame_override_wins() {
    let dir = TempDir::new("crop-batch");
    for (name, dmin) in [
        ("a.tif", [0.20, 0.50, 0.70]),
        ("b.tif", [0.21, 0.51, 0.71]),
        ("c.tif", [0.22, 0.52, 0.72]),
    ] {
        write_negative_tiff(
            &dir.path(&format!("roll/{name}")),
            96,
            64,
            dmin,
            [1.6, 1.7, 1.8],
        );
    }
    let roll = dir.path("roll");
    let o = negg(&["batch", roll.to_str().unwrap(), "--crop", "0.1,0.1,0.8,0.8"]);
    assert_eq!(o.status.code(), Some(0), "{}\n{}", stdout(&o), stderr(&o));
    let rs = read_json(&roll.join("roll.negg.json"));
    assert_eq!(rs["schema_version"], 3);
    assert_eq!(rs["settings"]["shared"]["schema_version"], 4);
    let crop = &rs["settings"]["shared"]["geometry"]["crop"];
    assert!((crop["x"].as_f64().unwrap() - 0.1).abs() < 1e-6, "{rs}");
    assert!((crop["width"].as_f64().unwrap() - 0.8).abs() < 1e-6, "{rs}");
    let roll_size = crop_geometry(0.1, 0.1, 0.8, 0.8).output_size(96, 64);
    for name in ["a", "b", "c"] {
        let (w, h, _) = read_tiff16(&roll.join(format!("{name}.negg.tif")));
        assert_eq!((w, h), roll_size, "{name}");
    }

    // A frame override with another crop wins on re-batch (no --crop: the
    // roll sidecar keeps the shared crop).
    let path = roll.join("b.negg.json");
    let mut s = read_json(&path);
    s["overrides"] = serde_json::json!({
        "geometry": {"crop": {"x": 0.25, "y": 0.25, "width": 0.5, "height": 0.5}}
    });
    std::fs::write(&path, serde_json::to_string_pretty(&s).unwrap()).unwrap();
    let o = negg(&["batch", roll.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{}\n{}", stdout(&o), stderr(&o));
    let own = crop_geometry(0.25, 0.25, 0.5, 0.5).output_size(96, 64);
    assert_ne!(own, roll_size);
    assert_eq!(read_tiff16(&roll.join("b.negg.tif")).0, own.0);
    assert_eq!(read_tiff16(&roll.join("b.negg.tif")).1, own.1);
    for name in ["a", "c"] {
        let (w, h, _) = read_tiff16(&roll.join(format!("{name}.negg.tif")));
        assert_eq!((w, h), roll_size, "{name}");
    }
    let b = read_json(&path);
    assert!(b["overrides"]["geometry"].is_object(), "{b}");
    assert_eq!(b["settings"]["geometry"]["crop"]["x"], 0.25);

    // A different crop on convert makes the stored pool stale.
    let a = roll.join("a.tif");
    let o = negg(&["convert", a.to_str().unwrap(), "--crop", "0,0,1,1"]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert!(
        stderr(&o).contains("its pool is not used"),
        "{}",
        stderr(&o)
    );
    let o = negg(&["convert", a.to_str().unwrap()]);
    assert!(
        stderr(&o).contains("using the roll settings and pool"),
        "{}",
        stderr(&o)
    );
}

#[test]
fn a_malformed_or_invalid_crop_is_a_usage_error() {
    let dir = TempDir::new("crop-usage");
    let input = dir.path("f/f.tif");
    write_negative_tiff(&input, 64, 48, [0.2, 0.5, 0.7], [1.7, 1.7, 1.6]);
    for bad in ["0.5,0.5,0.6,0.1", "1,2,3"] {
        for cmd in ["convert", "batch"] {
            let target = if cmd == "convert" {
                input.clone()
            } else {
                dir.path("f")
            };
            let o = negg(&[cmd, target.to_str().unwrap(), "--crop", bad]);
            assert_eq!(o.status.code(), Some(2), "{cmd} {bad}: {}", stderr(&o));
        }
    }
    assert!(!dir.path("f/f.negg.tif").exists());
}
