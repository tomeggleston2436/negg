// SPDX-License-Identifier: Apache-2.0

//! End-to-end tests for roll mode (`negg batch` and `negg convert` with a
//! roll sidecar), `--no-roll` and `negg lock`, on synthetic negatives.

mod common;

use std::path::Path;
use std::process::{Command, Output};

use common::{TempDir, read_json, without_icc_date, write_negative_tiff};
use tiff::encoder::TiffEncoder;
use tiff::encoder::colortype::RGB16;

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

/// A roll of four frames in `dir/roll`: three with nearly the same film
/// base and one whose base is 0.5 log-D denser.
fn write_roll(dir: &TempDir) {
    let bases = [
        ("a.tif", [0.20, 0.50, 0.70]),
        ("b.tif", [0.21, 0.51, 0.71]),
        ("c.tif", [0.22, 0.52, 0.72]),
        ("d.tif", [0.70, 1.00, 1.20]),
    ];
    for (name, dmin) in bases {
        write_negative_tiff(
            &dir.path(&format!("roll/{name}")),
            64,
            48,
            dmin,
            [1.6, 1.7, 1.8],
        );
    }
}

#[test]
fn batch_pools_the_roll_and_rejects_the_outlier() {
    let dir = TempDir::new("roll-pool");
    write_roll(&dir);
    let roll = dir.path("roll");
    let o = negg(&["batch", roll.to_str().unwrap(), "--dump-stats"]);
    let (so, se) = (stdout(&o), stderr(&o));
    assert_eq!(o.status.code(), Some(0), "{so}\n{se}");
    assert!(so.ends_with("converted 4 of 4 frames (0 failed)\n"), "{so}");
    assert!(so.contains("roll roles: dmin inlier, gamma inlier"), "{so}");
    assert!(so.contains("roll roles: dmin rejected"), "{so}");
    assert!(so.contains("cast source: roll"), "{so}");
    assert!(se.contains("d.tif: rejected from the Dmin pool"), "{se}");

    let rs = read_json(&roll.join("roll.negg.json"));
    assert_eq!(rs["schema_version"], 1);
    assert_eq!(
        rs["frames"],
        serde_json::json!(["a.tif", "b.tif", "c.tif", "d.tif"])
    );
    let frames = rs["pool"]["frames"].as_array().unwrap();
    let roles: Vec<&str> = frames
        .iter()
        .map(|f| f["dmin_role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, ["inlier", "inlier", "inlier", "rejected"]);
    assert_eq!(frames[3]["cast_role"], "rejected");
    let pooled = &rs["pool"]["dmin"];
    assert!(pooled.is_array(), "{rs}");
    // The pooled base is the inliers' median, close to b's base.
    for (c, want) in [0.21, 0.51, 0.71].iter().enumerate() {
        let got = pooled[c].as_f64().unwrap();
        assert!((got - want).abs() < 0.02, "{pooled}");
    }
    for name in ["a", "b", "c"] {
        let s = read_json(&roll.join(format!("{name}.negg.json")));
        assert_eq!(s["estimates"]["dmin"], *pooled, "{name}");
        assert_eq!(s["estimates"]["dmin_source"], "roll", "{name}");
        assert_eq!(s["estimates"]["gamma_source"], "roll", "{name}");
        assert_eq!(s["estimates"]["gamma"], rs["pool"]["gamma"], "{name}");
        assert_eq!(s["estimates"]["cast_source"], "roll", "{name}");
        assert_eq!(s["roll"], "roll.negg.json", "{name}");
        assert_eq!(s["schema_version"], 2);
        assert!(s.get("overrides").is_none(), "{s}");
    }
    // The Rejected frame still renders with the pooled base (Ruling 59).
    let d = read_json(&roll.join("d.negg.json"));
    assert_eq!(d["estimates"]["dmin"], *pooled, "{d}");
    assert_eq!(d["estimates"]["dmin_source"], "roll", "{d}");
    // Re-running reads the roll sidecar and gives the same pool.
    let o = negg(&["batch", roll.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let again = read_json(&roll.join("roll.negg.json"));
    assert_eq!(again["pool"], rs["pool"]);
}

/// The input of the phase 1 golden file: a 64 by 48 synthetic negative
/// written as a 16-bit RGB TIFF with no ICC profile. Kept here rather than
/// in `tests/common` so that the golden file's input cannot drift with the
/// shared helpers: an unexposed border of density (0.20, 0.50, 0.70)
/// around an interior ramping by (1.6, 1.7, 1.8) log-D with a checker
/// texture, transmittance stored gamma 2.2 encoded.
fn write_golden_input(path: &Path) {
    const W: usize = 64;
    const H: usize = 48;
    const DMIN: [f32; 3] = [0.20, 0.50, 0.70];
    const GAMMA: [f32; 3] = [1.6, 1.7, 1.8];
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let border = W.min(H) / 8;
    let mut data = Vec::with_capacity(W * H * 3);
    for y in 0..H {
        for x in 0..W {
            let interior = x >= border && x < W - border && y >= border && y < H - border;
            let t = if interior {
                let ramp = (x - border) as f32 / (W - 2 * border) as f32;
                let texture = if (x / 2 + y / 2) % 2 == 0 { 0.0 } else { 0.08 };
                (ramp * 0.9 + texture).min(1.0)
            } else {
                0.0
            };
            for c in 0..3 {
                let transmittance = 10f32.powf(-(DMIN[c] + GAMMA[c] * t));
                data.push((transmittance.powf(1.0 / 2.2) * 65535.0).round() as u16);
            }
        }
    }
    let mut enc = TiffEncoder::new(std::fs::File::create(path).unwrap()).unwrap();
    enc.write_image::<RGB16>(W as u32, H as u32, &data).unwrap();
}

/// The golden output of the phase 1 binary for [`write_golden_input`]:
/// `negg convert golden.tif --out <dir>` with the default settings, built
/// from the pre-phase-2 snapshot (`.superpowers/sdd/snap-p2w1`, `cargo
/// build --release --locked`). To regenerate it, write the input with
/// `cargo test -p negg-cli --test roll -- --ignored write_the_golden_input`
/// (it goes to `$NEGG_GOLDEN_INPUT`) and convert it with that binary.
const PHASE1_GOLDEN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/data/golden.phase1.negg.tif"
);

#[test]
#[ignore = "writes the golden input for regeneration; needs NEGG_GOLDEN_INPUT"]
fn write_the_golden_input() {
    let path = std::env::var("NEGG_GOLDEN_INPUT").expect("NEGG_GOLDEN_INPUT");
    write_golden_input(Path::new(&path));
}

#[test]
fn no_roll_matches_the_phase_1_binary_byte_for_byte() {
    let dir = TempDir::new("no-roll");
    let roll = dir.path("roll");
    write_golden_input(&roll.join("golden.tif"));
    let want = without_icc_date(Path::new(PHASE1_GOLDEN));

    // `negg batch --no-roll` is phase 1 and writes no roll sidecar.
    let out = dir.path("batch");
    let o = negg(&[
        "batch",
        roll.to_str().unwrap(),
        "--no-roll",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert!(!roll.join("roll.negg.json").exists());
    assert!(without_icc_date(&out.join("golden.negg.tif")) == want);
    let s = read_json(&roll.join("golden.negg.json"));
    assert!(s.get("roll").is_none(), "{s}");
    assert_ne!(s["estimates"]["dmin_source"], "roll");

    // `negg convert` with no roll sidecar is phase 1 too.
    let out = dir.path("convert");
    let o = negg(&[
        "convert",
        roll.join("golden.tif").to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert!(without_icc_date(&out.join("golden.negg.tif")) == want);
}

#[test]
fn roll_mode_renders_a_pooled_outlier_differently_from_no_roll() {
    let dir = TempDir::new("roll-vs-no-roll");
    write_roll(&dir);
    let roll = dir.path("roll");
    let (plain, pooled) = (dir.path("plain"), dir.path("pooled"));
    for (out, extra) in [(&plain, Some("--no-roll")), (&pooled, None)] {
        let mut args = vec![
            "batch",
            roll.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ];
        args.extend(extra);
        let o = negg(&args);
        assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    }
    assert_ne!(
        without_icc_date(&pooled.join("d.negg.tif")),
        without_icc_date(&plain.join("d.negg.tif"))
    );
}

#[test]
fn convert_uses_the_roll_sidecar_and_never_rewrites_it() {
    let dir = TempDir::new("convert-roll");
    write_roll(&dir);
    let roll = dir.path("roll");
    let o = negg(&["batch", roll.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let roll_bytes = std::fs::read(roll.join("roll.negg.json")).unwrap();
    let pooled = read_json(&roll.join("roll.negg.json"))["pool"]["dmin"].clone();

    let a = roll.join("a.tif");
    let o = negg(&["convert", a.to_str().unwrap(), "--dump-stats"]);
    let (so, se) = (stdout(&o), stderr(&o));
    assert_eq!(o.status.code(), Some(0), "{so}\n{se}");
    assert!(se.contains("using the roll settings and pool"), "{se}");
    assert!(so.contains("roll roles: dmin inlier"), "{so}");
    let s = read_json(&roll.join("a.negg.json"));
    assert_eq!(s["estimates"]["dmin"], pooled);
    assert_eq!(s["roll"], "roll.negg.json");
    assert_eq!(
        std::fs::read(roll.join("roll.negg.json")).unwrap(),
        roll_bytes
    );

    let o = negg(&["convert", a.to_str().unwrap(), "--no-roll"]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let s = read_json(&roll.join("a.negg.json"));
    assert_ne!(s["estimates"]["dmin_source"], "roll");
    assert!(s.get("roll").is_none());
    assert_eq!(
        std::fs::read(roll.join("roll.negg.json")).unwrap(),
        roll_bytes
    );

    // A frame the roll sidecar does not list is converted on its own.
    write_negative_tiff(&roll.join("e.tif"), 64, 48, [0.2, 0.5, 0.7], [1.6; 3]);
    let o = negg(&["convert", roll.join("e.tif").to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert!(!stderr(&o).contains("using the roll"), "{}", stderr(&o));
    assert!(read_json(&roll.join("e.negg.json")).get("roll").is_none());
}

#[test]
fn lock_then_rebatch_keeps_the_locked_values() {
    let dir = TempDir::new("lock");
    write_roll(&dir);
    let roll = dir.path("roll");
    // Convert c on its own first, so its own estimates differ from the
    // pool, then lock them.
    let c = roll.join("c.tif");
    let o = negg(&["convert", c.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let before = read_json(&roll.join("c.negg.json"))["estimates"].clone();
    let o = negg(&["lock", c.to_str().unwrap()]);
    let so = stdout(&o);
    assert_eq!(o.status.code(), Some(0), "{so}\n{}", stderr(&o));
    assert!(so.starts_with("locked dmin, gamma, cast in "), "{so}");
    let s = read_json(&roll.join("c.negg.json"));
    assert_eq!(s["overrides"]["film"]["film_base"]["mode"], "fixed");
    assert_eq!(s["overrides"]["film"]["film_base"]["value"], before["dmin"]);
    assert_eq!(s["overrides"]["film"]["gamma"]["value"], before["gamma"]);
    assert_eq!(s["overrides"]["colour"]["cast"]["mode"], "fixed");

    let o = negg(&["batch", roll.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let s = read_json(&roll.join("c.negg.json"));
    assert_eq!(s["estimates"]["dmin"], before["dmin"]);
    assert_eq!(s["estimates"]["dmin_source"], "fixed");
    assert_eq!(s["estimates"]["gamma"], before["gamma"]);
    assert_eq!(s["estimates"]["gamma_source"], "fixed");
    assert_eq!(s["estimates"]["cast_source"], "fixed");
    assert_eq!(s["overrides"]["film"]["film_base"]["value"], before["dmin"]);
    let rs = read_json(&roll.join("roll.negg.json"));
    let fc = &rs["pool"]["frames"][2];
    assert_eq!(fc["source"], "c.tif");
    assert_eq!(fc["dmin_role"], "excluded");
    assert_eq!(fc["gamma_role"], "excluded");
    assert_eq!(fc["cast_role"], "excluded");
    // The others still take the pool, which c no longer feeds.
    let a = read_json(&roll.join("a.negg.json"));
    assert_eq!(a["estimates"]["dmin"], rs["pool"]["dmin"]);

    // Lock only the gamma of a.
    let o = negg(&["lock", roll.join("a.tif").to_str().unwrap(), "--gamma"]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let a = read_json(&roll.join("a.negg.json"));
    assert!(
        a["overrides"]["film"]
            .get("film_base")
            .is_none_or(|v| v.is_null())
    );
    assert_eq!(a["overrides"]["film"]["gamma"]["mode"], "fixed");

    // A frame with no sidecar cannot be locked.
    let o = negg(&["lock", roll.join("missing.tif").to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1));
}

#[test]
fn a_frame_whose_sidecar_would_be_the_roll_sidecar_is_refused() {
    let dir = TempDir::new("roll-name");
    let roll = dir.path("roll");
    write_negative_tiff(&roll.join("roll.tif"), 32, 24, [0.2; 3], [1.6; 3]);
    write_negative_tiff(&roll.join("a.tif"), 32, 24, [0.2; 3], [1.6; 3]);
    let o = negg(&["batch", roll.to_str().unwrap()]);
    let (so, se) = (stdout(&o), stderr(&o));
    assert_eq!(o.status.code(), Some(1), "{so}\n{se}");
    assert!(so.ends_with("converted 1 of 2 frames (1 failed)\n"), "{so}");
    assert!(se.contains("roll.tif: refusing"), "{se}");
    assert!(!roll.join("roll.negg.tif").exists());
    let rs = negg_io::read_roll_sidecar(&roll.join("roll.negg.json")).unwrap();
    assert_eq!(rs.frames, ["a.tif"]);

    let o = negg(&[
        "convert",
        roll.join("roll.tif").to_str().unwrap(),
        "--no-roll",
    ]);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        stderr(&o).contains("roll sidecar roll.negg.json"),
        "{}",
        stderr(&o)
    );
}

#[test]
fn a_bad_profile_flag_is_a_usage_error() {
    let dir = TempDir::new("bad-profile");
    write_negative_tiff(&dir.path("roll/a.tif"), 32, 24, [0.2; 3], [1.6; 3]);
    let roll = dir.path("roll");
    let missing = dir.path("missing.rig.negg.toml");
    let o = negg(&[
        "batch",
        roll.to_str().unwrap(),
        "--rig",
        missing.to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
    assert!(
        stderr(&o).contains("negg batch: profile "),
        "{}",
        stderr(&o)
    );
    dir.write("bad.film.negg.toml", b"schema_version = 1\n");
    let o = negg(&[
        "convert",
        roll.join("a.tif").to_str().unwrap(),
        "--film",
        dir.path("bad.film.negg.toml").to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
}

#[test]
fn a_raw_named_roll_is_refused_before_it_is_decoded() {
    let dir = TempDir::new("roll-dng");
    write_roll(&dir);
    // Never decoded: the name alone is refused, so the bytes do not matter.
    dir.write("roll/roll.dng", b"not a raw file");
    let roll = dir.path("roll");
    let o = negg(&["batch", roll.to_str().unwrap()]);
    let (so, se) = (stdout(&o), stderr(&o));
    assert_eq!(o.status.code(), Some(1), "{so}\n{se}");
    assert!(so.ends_with("converted 4 of 5 frames (1 failed)\n"), "{so}");
    assert!(se.contains("roll.dng: refusing"), "{se}");
    let rs = negg_io::read_roll_sidecar(&roll.join("roll.negg.json")).unwrap();
    assert_eq!(rs.frames, ["a.tif", "b.tif", "c.tif", "d.tif"]);
    assert_eq!(rs.schema_version, 1);
}

#[test]
fn a_frame_failing_in_pass_1_is_left_out_of_the_pool_and_pass_2() {
    let dir = TempDir::new("roll-pass1");
    write_roll(&dir);
    dir.write("roll/broken.tif", b"not a tiff");
    let roll = dir.path("roll");
    let o = negg(&["batch", roll.to_str().unwrap()]);
    let (so, se) = (stdout(&o), stderr(&o));
    assert_eq!(o.status.code(), Some(1), "{so}\n{se}");
    assert!(so.ends_with("converted 4 of 5 frames (1 failed)\n"), "{so}");
    assert!(se.contains("broken.tif"), "{se}");
    // Reported once, in pass 1, and never rendered.
    assert_eq!(se.matches("broken.tif").count(), 1, "{se}");
    assert!(!roll.join("broken.negg.tif").exists());
    assert!(!roll.join("broken.negg.json").exists());
    let rs = negg_io::read_roll_sidecar(&roll.join("roll.negg.json")).unwrap();
    assert_eq!(rs.frames, ["a.tif", "b.tif", "c.tif", "d.tif"]);
    let pool = rs.pool.unwrap();
    assert_eq!(pool.frames.len(), 4);
    assert!(pool.dmin.is_some());
}

#[test]
fn an_unreadable_frame_sidecar_fails_the_frame_and_is_kept() {
    let dir = TempDir::new("bad-sidecar");
    write_roll(&dir);
    let roll = dir.path("roll");
    let o = negg(&["batch", roll.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let a = roll.join("a.tif");
    let o = negg(&["lock", a.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    // A hand edit leaves a trailing comma inside the JSON.
    let sidecar = roll.join("a.negg.json");
    let text = std::fs::read_to_string(&sidecar).unwrap();
    let broken = text.replacen('{', "{\"stray\": 1,,", 1);
    std::fs::write(&sidecar, &broken).unwrap();
    let output = roll.join("a.negg.tif");
    let output_bytes = std::fs::read(&output).unwrap();
    let others: Vec<_> = ["b", "c", "d"]
        .iter()
        .map(|n| {
            std::fs::metadata(roll.join(format!("{n}.negg.tif")))
                .unwrap()
                .modified()
                .unwrap()
        })
        .collect();

    let o = negg(&["batch", roll.to_str().unwrap()]);
    let (so, se) = (stdout(&o), stderr(&o));
    assert_eq!(o.status.code(), Some(1), "{so}\n{se}");
    assert!(so.ends_with("converted 3 of 4 frames (1 failed)\n"), "{so}");
    assert!(se.contains("cannot read the existing sidecar"), "{se}");
    assert_eq!(std::fs::read_to_string(&sidecar).unwrap(), broken);
    assert_eq!(std::fs::read(&output).unwrap(), output_bytes);
    // The other frames are rendered again, and the roll leaves a out.
    for (n, before) in ["b", "c", "d"].iter().zip(others) {
        let after = std::fs::metadata(roll.join(format!("{n}.negg.tif")))
            .unwrap()
            .modified()
            .unwrap();
        assert!(after >= before, "{n}");
        assert_eq!(
            read_json(&roll.join(format!("{n}.negg.json")))["roll"],
            "roll.negg.json"
        );
    }
    let rs = negg_io::read_roll_sidecar(&roll.join("roll.negg.json")).unwrap();
    assert_eq!(rs.frames, ["b.tif", "c.tif", "d.tif"]);

    // `negg convert` refuses the frame too, in and out of roll mode.
    for extra in [None, Some("--no-roll")] {
        let mut args = vec!["convert", a.to_str().unwrap()];
        args.extend(extra);
        let o = negg(&args);
        assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
        assert!(
            stderr(&o).contains("cannot read the existing sidecar"),
            "{}",
            stderr(&o)
        );
        assert_eq!(std::fs::read_to_string(&sidecar).unwrap(), broken);
        assert_eq!(std::fs::read(&output).unwrap(), output_bytes);
    }
}

/// An identity rig profile with a density model whose offset `a` is 0.5,
/// so densities are 0.5 log-D above the plain formula.
const OFFSET_RIG: &str = r#"schema_version = 1
name = "offset"
source = "synthetic density offset for the stale pool test"
measured_on = "2026-10-23"
method = ["density_fit"]
residual_rms = 0.0

[density_fit]
method = "density_fit"
source = "synthetic, a = 0.5 log-D"
measured_on = "2026-10-23"
residual_rms = 0.0
n_patches = 3
model = "diagonal"
a = [0.5, 0.5, 0.5]
b = [-1.0, -1.0, -1.0]
c = [[0.0, 0.0, 0.0], [0.0, 0.0, 0.0], [0.0, 0.0, 0.0]]
"#;

#[test]
fn convert_with_another_rig_does_not_use_a_stale_pool() {
    let dir = TempDir::new("stale-pool");
    write_roll(&dir);
    let roll = dir.path("roll");
    let o = negg(&["batch", roll.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let roll_bytes = std::fs::read(roll.join("roll.negg.json")).unwrap();
    dir.write("offset.rig.negg.toml", OFFSET_RIG.as_bytes());
    let rig = dir.path("offset.rig.negg.toml");

    let a = roll.join("a.tif");
    let o = negg(&[
        "convert",
        a.to_str().unwrap(),
        "--rig",
        rig.to_str().unwrap(),
        "--dump-stats",
    ]);
    let (so, se) = (stdout(&o), stderr(&o));
    assert_eq!(o.status.code(), Some(0), "{so}\n{se}");
    assert!(se.contains("its pool is not used"), "{se}");
    assert!(se.contains("batch the roll again"), "{se}");
    assert!(!se.contains("using the roll settings and pool"), "{se}");
    assert!(!so.contains("roll roles"), "{so}");
    let s = read_json(&roll.join("a.negg.json"));
    assert_ne!(s["estimates"]["dmin_source"], "roll", "{s}");
    assert!(s.get("roll").is_none(), "{s}");
    // The frame's own base is measured through the new rig: 0.5 log-D up.
    let dmin = s["estimates"]["dmin"][0].as_f64().unwrap();
    assert!((dmin - 0.70).abs() < 0.05, "{s}");
    assert_eq!(
        std::fs::read(roll.join("roll.negg.json")).unwrap(),
        roll_bytes
    );

    // Without a profile change the pool is still used.
    let o = negg(&["convert", a.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert!(stderr(&o).contains("using the roll settings and pool"));
    assert_eq!(
        read_json(&roll.join("a.negg.json"))["estimates"]["dmin_source"],
        "roll"
    );
}
