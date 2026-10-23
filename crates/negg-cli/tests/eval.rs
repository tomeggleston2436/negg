// SPDX-License-Identifier: Apache-2.0

//! End-to-end tests for `negg eval` through the built binary.
//!
//! The corpus frames are synthetic negatives written as untagged 16-bit
//! TIFFs (see `common`), which decode as `Gamma22Assumed` and go through
//! the whole pipeline; the reference renders are plain grey TIFFs.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use common::{TempDir, write_grey_tiff, write_negative_tiff};

const MANIFEST: &str = r#"schema_version = 1

[[frame]]
id = "r001-01"
file = "frames/r001/r001-01.tif"
roll = "r001"
stock = "Kodak Portra 400"
camera = "Fujifilm X-T4"
light_source = "Cinestill CS-Lite"
has_grey_card = true
grey_card = { x = 0.25, y = 0.25, width = 0.5, height = 0.5 }
references = [
  { tool = "negpy", file = "reference/r001-01.negpy.tif", version = "0.3.1" },
]
notes = ""
contributor = "Test"
licence = "CC0-1.0"
"#;

/// The manifest for `one_frame_corpus` plus a second frame on the same
/// roll with no references.
const SECOND_FRAME: &str = r#"
[[frame]]
id = "r001-02"
file = "frames/r001/r001-02.tif"
roll = "r001"
stock = "Kodak Portra 400"
camera = "Fujifilm X-T4"
light_source = "Cinestill CS-Lite"
has_grey_card = false
references = []
notes = ""
contributor = "Test"
licence = "CC0-1.0"
"#;

/// The frame written for `r001-01`: 64x48.
fn write_frame(path: &Path) {
    write_negative_tiff(path, 64, 48, [0.2, 0.5, 0.7], [1.7, 1.7, 1.6]);
}

/// A corpus with one 64x48 frame, one grey card and one 32x24 reference
/// render (half size, so the comparison goes through the shared analysis
/// grid).
fn one_frame_corpus(tag: &str) -> TempDir {
    let dir = TempDir::new(tag);
    write_frame(&dir.path("frames/r001/r001-01.tif"));
    write_grey_tiff(&dir.path("reference/r001-01.negpy.tif"), 32, 24);
    dir.write("manifest.toml", MANIFEST.as_bytes());
    dir
}

fn run_eval(corpus: &Path, out: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_negg"))
        .args(["eval", "--corpus"])
        .arg(corpus)
        .arg("--out")
        .arg(out)
        .arg("--quiet")
        .output()
        .expect("run negg")
}

fn run_eval_table(corpus: &Path, out: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_negg"))
        .args(["eval", "--corpus"])
        .arg(corpus)
        .arg("--out")
        .arg(out)
        .output()
        .expect("run negg")
}

fn read_json(path: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn one_frame_is_rendered_and_scored() {
    let dir = one_frame_corpus("one");
    let out = dir.path("eval-results.json");
    let output = run_eval(&dir.0, &out);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty(), "quiet should print nothing");

    let text = std::fs::read_to_string(&out).unwrap();
    assert!(text.ends_with('\n'));
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["schema_version"], 2);
    assert_eq!(v["corpus"]["frames"], 1);
    assert_eq!(v["corpus"]["evaluated"], 1);
    assert_eq!(v["corpus"]["with_references"], 1);
    assert_eq!(v["corpus"]["with_grey_card"], 1);
    let f = &v["frames"][0];
    assert_eq!(f["id"], "r001-01");
    assert_eq!(f["references"][0]["tool"], "negpy");
    // A real render against a flat grey reference: present and finite,
    // not zero.
    for key in ["mean", "p95", "max"] {
        let de = f["references"][0]["de2000"][key]
            .as_f64()
            .unwrap_or_else(|| panic!("de2000 {key} in {f}"));
        assert!(de.is_finite() && de >= 0.0, "{key} = {de}");
    }
    assert!(f["neutrality"].is_object(), "{f}");
    assert!(f["neutrality"]["a_mean_abs"].as_f64().unwrap().is_finite());
    assert!(f["neutrality"]["de"].as_f64().unwrap().is_finite());
    // One reference only: no reference distance, and Gate 1 cannot be
    // judged.
    assert!(f["reference_distance"].is_null(), "{f}");
    assert!(v["summary"]["gate1"]["engine_vs_negpy"].is_null());
    assert!(v["summary"]["gate1"]["pass"].is_null());
    assert!(v["summary"]["gate1"]["neutrality_de_mean"].is_number());
    assert!(v["summary"]["de2000_mean_by_tool"]["negpy"].is_number());
    assert!(v["summary"]["de2000_mean_by_tool"]["negadoctor"].is_null());
    let e = &f["estimates"];
    assert!(e.is_object(), "{f}");
    assert!(e["dmin_source"].is_string(), "{e}");
    // Every frame is rendered in roll mode (Ruling 53), so even a roll of
    // one takes its gamma from the pool, which holds its own value.
    assert_eq!(e["gamma_source"], "roll");
    assert_eq!(f["pool"]["dmin_role"], "inlier", "{f}");
    assert_eq!(f["pool"]["dmin"].as_array().unwrap().len(), 3);
    assert!(f["film_roll"].is_null() && f["rig"].is_null(), "{f}");
    assert_eq!(e["dmin"].as_array().unwrap().len(), 3);
    assert!(e["x0"].as_f64().unwrap().is_finite());
    assert!(e["cast"]["slope_r"].is_number());
    assert_eq!(v["rolls"].as_array().unwrap().len(), 1);
    assert_eq!(v["rolls"][0]["roll"], "r001");
    assert_eq!(v["rolls"][0]["frames"], 1);
    assert!(v["rolls"][0]["dmin_variance"].is_null());
    assert!(v["rolls"][0]["gamma_variance"].is_null());
    assert_eq!(v["rolls"][0]["inliers"], 1);
    assert!(v["rolls"][0]["dmin_std"].is_null());
    assert_eq!(v["rolls"][0]["pooled_dmin"], e["dmin"]);
    assert!(v["rolls"][0]["pooled_gamma_ratio"].is_array());
    assert_eq!(v["roll_to_roll"], serde_json::json!([]));
    assert_eq!(v["three_lights"], serde_json::json!([]));
    assert!(v["summary"]["gate2"]["dmin_std_max"].is_null());
    assert!(v["summary"]["gate2"]["pass"].is_null());
    assert!(v["summary"]["de2000_mean"].is_number());
    assert!(v["summary"]["de2000_p95_mean"].is_number());
    assert!(v["summary"]["neutrality_mean"].is_number());
    assert_eq!(v["skipped"], serde_json::json!([]));
    // Diffable: nothing in the file names the temp directory.
    assert!(!text.contains(dir.0.to_str().unwrap()));
}

#[test]
fn both_references_give_a_reference_distance_and_the_gate1_summary() {
    // Ruling 32: with a negadoctor and a NegPy reference the frame gets the
    // reference-to-reference distance and the summary the Gate 1 fields.
    let dir = one_frame_corpus("gate1");
    write_grey_tiff(&dir.path("reference/r001-01.negadoctor.tif"), 64, 48);
    let manifest = MANIFEST.replace(
        r#"  { tool = "negpy", file = "reference/r001-01.negpy.tif", version = "0.3.1" },"#,
        r#"  { tool = "negpy", file = "reference/r001-01.negpy.tif", version = "0.3.1" },
  { tool = "negadoctor", file = "reference/r001-01.negadoctor.tif", version = "4.8" },"#,
    );
    dir.write("manifest.toml", manifest.as_bytes());
    let out = dir.path("eval-results.json");
    let output = run_eval_table(&dir.0, &out);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{stdout}");
    let v = read_json(&out);
    let f = &v["frames"][0];
    // The two references are the same flat grey (at different sizes, so
    // matched on the shared grid): distance zero.
    assert_eq!(f["reference_distance"]["mean"], 0.0, "{f}");
    let negpy = f["references"][0]["de2000"]["mean"].as_f64().unwrap();
    let negadoctor = f["references"][1]["de2000"]["mean"].as_f64().unwrap();
    let s = &v["summary"];
    let close = |a: &serde_json::Value, b: f64| (a.as_f64().unwrap() - b).abs() < 1e-6;
    assert!(close(&s["de2000_mean_by_tool"]["negpy"], negpy), "{s}");
    assert!(
        close(&s["de2000_mean_by_tool"]["negadoctor"], negadoctor),
        "{s}"
    );
    let g = &s["gate1"];
    assert!(close(&g["engine_vs_negpy"], negpy), "{g}");
    assert_eq!(g["negadoctor_vs_negpy"], 0.0, "{g}");
    let de = f["neutrality"]["de"].as_f64().unwrap();
    assert!(close(&g["neutrality_de_mean"], de), "{g}");
    // The engine is farther from NegPy than an identical reference is.
    assert_eq!(g["pass"], false, "{g}");
    assert!(stdout.contains("gate 1: engine vs negpy "), "{stdout}");
    assert!(stdout.contains("pass no"), "{stdout}");
}

#[test]
fn one_frame_prints_a_table_when_not_quiet() {
    let dir = one_frame_corpus("table");
    let out = dir.path("eval-results.json");
    let output = run_eval_table(&dir.0, &out);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("r001-01"), "{stdout}");
    assert!(stdout.contains("negpy"), "{stdout}");
    assert!(stdout.lines().any(|l| l.starts_with("roll")), "{stdout}");
    assert!(
        stdout.lines().any(|l| l.starts_with("summary:")),
        "{stdout}"
    );
    // The skipped line is printed even when nothing was skipped.
    assert_eq!(
        stdout.lines().last().unwrap(),
        "Skipped 0 frame(s) and 0 reference render(s)",
        "{stdout}"
    );
}

#[test]
fn two_frames_on_a_roll_give_per_channel_variances() {
    let dir = one_frame_corpus("roll");
    write_negative_tiff(
        &dir.path("frames/r001/r001-02.tif"),
        64,
        48,
        [0.25, 0.55, 0.75],
        [1.6, 1.6, 1.5],
    );
    dir.write(
        "manifest.toml",
        format!("{MANIFEST}{SECOND_FRAME}").as_bytes(),
    );
    let out = dir.path("eval-results.json");
    let output = run_eval(&dir.0, &out);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let v = read_json(&out);
    assert_eq!(v["corpus"]["evaluated"], 2);
    assert_eq!(v["rolls"][0]["frames"], 2);
    let dmin = v["rolls"][0]["dmin_variance"].as_array().expect("array");
    let gamma = v["rolls"][0]["gamma_variance"].as_array().expect("array");
    assert_eq!(dmin.len(), 3);
    assert_eq!(gamma.len(), 3);
    for c in 0..3 {
        let d = dmin[c].as_f64().unwrap();
        assert!(d.is_finite() && d > 0.0, "dmin variance {dmin:?}");
        assert!(gamma[c].as_f64().unwrap().is_finite());
    }
    // The second frame has no card and no references but still carries
    // its estimates.
    assert!(v["frames"][1]["neutrality"].is_null());
    assert!(v["frames"][1]["estimates"].is_object());
}

#[test]
fn frame_with_missing_file_is_skipped_and_counted() {
    let dir = one_frame_corpus("skipframe");
    // r001-02 is in the manifest but its scan was never placed locally.
    dir.write(
        "manifest.toml",
        format!("{MANIFEST}{SECOND_FRAME}").as_bytes(),
    );
    let out = dir.path("eval-results.json");
    let output = run_eval_table(&dir.0, &out);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("Skipped 1 frame(s) and 0 reference render(s)"),
        "{stdout}"
    );
    assert!(
        stdout.contains("  r001-02: file not found: frames/r001/r001-02.tif"),
        "{stdout}"
    );
    assert!(stdout.contains("r001-01"), "{stdout}");

    let text = std::fs::read_to_string(&out).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["corpus"]["frames"], 2);
    assert_eq!(v["corpus"]["evaluated"], 1);
    assert_eq!(v["corpus"]["with_references"], 1);
    assert_eq!(v["corpus"]["with_grey_card"], 1);
    assert_eq!(v["corpus"]["skipped_frames"], 1);
    assert_eq!(v["corpus"]["skipped_references"], 0);
    assert_eq!(v["frames"].as_array().unwrap().len(), 1);
    assert_eq!(v["frames"][0]["id"], "r001-01");
    assert_eq!(v["rolls"][0]["frames"], 1);
    assert_eq!(v["skipped"].as_array().unwrap().len(), 1);
    assert_eq!(v["skipped"][0]["id"], "r001-02");
    assert_eq!(
        v["skipped"][0]["reason"],
        "file not found: frames/r001/r001-02.tif"
    );
    // Relative paths only.
    assert!(!text.contains(dir.0.to_str().unwrap()));

    // Under --quiet the table is not printed, but the non-zero count is.
    let output = run_eval(&dir.0, &out);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout, "Skipped 1 frame(s) and 0 reference render(s)\n");
}

#[test]
fn frame_that_does_not_decode_is_skipped_with_the_reason() {
    let dir = one_frame_corpus("baddecode");
    dir.write("frames/r001/r001-02.tif", b"not a tiff at all");
    dir.write(
        "manifest.toml",
        format!("{MANIFEST}{SECOND_FRAME}").as_bytes(),
    );
    let out = dir.path("eval-results.json");
    let output = run_eval_table(&dir.0, &out);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("Skipped 1 frame(s) and 0 reference render(s)"),
        "{stdout}"
    );
    assert!(stdout.contains("  r001-02: decode failed: "), "{stdout}");
    let v = read_json(&out);
    assert_eq!(v["corpus"]["evaluated"], 1);
    assert_eq!(v["corpus"]["skipped_frames"], 1);
    assert_eq!(v["skipped"][0]["id"], "r001-02");
    assert!(
        v["skipped"][0]["reason"]
            .as_str()
            .unwrap()
            .starts_with("decode failed: "),
        "{}",
        v["skipped"][0]
    );
}

#[test]
fn frame_with_missing_reference_is_evaluated_without_it() {
    let dir = one_frame_corpus("skipref");
    std::fs::remove_file(dir.path("reference/r001-01.negpy.tif")).unwrap();
    let out = dir.path("eval-results.json");
    let output = run_eval_table(&dir.0, &out);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("Skipped 0 frame(s) and 1 reference render(s)"),
        "{stdout}"
    );
    assert!(
        stdout.contains("  r001-01: reference not found: reference/r001-01.negpy.tif"),
        "{stdout}"
    );

    let v = read_json(&out);
    assert_eq!(v["corpus"]["frames"], 1);
    assert_eq!(v["corpus"]["evaluated"], 1);
    assert_eq!(v["corpus"]["with_references"], 0);
    assert_eq!(v["corpus"]["with_grey_card"], 1);
    assert_eq!(v["corpus"]["skipped_frames"], 0);
    assert_eq!(v["corpus"]["skipped_references"], 1);
    let f = &v["frames"][0];
    assert_eq!(f["id"], "r001-01");
    assert_eq!(f["references"], serde_json::json!([]));
    // Neutrality is measured on the render, so it needs no reference.
    assert!(f["neutrality"].is_object(), "{f}");
    assert!(f["estimates"].is_object(), "{f}");
    assert!(v["summary"]["de2000_mean"].is_null());
    assert!(v["summary"]["neutrality_mean"].is_number());
    assert_eq!(v["skipped"][0]["id"], "r001-01");
    assert_eq!(
        v["skipped"][0]["reason"],
        "reference not found: reference/r001-01.negpy.tif"
    );
}

#[test]
fn reference_of_an_unmatchable_size_is_skipped() {
    let dir = one_frame_corpus("sizemismatch");
    // 16x16 against a 64x48 render: the shared grid is 16x12 against
    // 16x16, more than one cell apart.
    write_grey_tiff(&dir.path("reference/r001-01.negpy.tif"), 16, 16);
    let out = dir.path("eval-results.json");
    let output = run_eval_table(&dir.0, &out);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("Skipped 0 frame(s) and 1 reference render(s)"),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "  r001-01: size mismatch: reference/r001-01.negpy.tif is 16x16 but the render is 64x48"
        ),
        "{stdout}"
    );
    let v = read_json(&out);
    assert_eq!(v["corpus"]["with_references"], 0);
    assert_eq!(v["corpus"]["skipped_references"], 1);
    assert_eq!(v["frames"][0]["references"], serde_json::json!([]));

    // The same size as the render compares pixel by pixel.
    write_grey_tiff(&dir.path("reference/r001-01.negpy.tif"), 64, 48);
    let output = run_eval(&dir.0, &out);
    assert_eq!(output.status.code(), Some(0));
    let v = read_json(&out);
    assert_eq!(v["corpus"]["with_references"], 1);
    assert!(v["frames"][0]["references"][0]["de2000"]["mean"].is_number());
}

#[test]
fn all_frames_skipped_prints_headers_and_writes_json() {
    let dir = TempDir::new("allskipped");
    dir.write("manifest.toml", MANIFEST.as_bytes());
    let out = dir.path("eval-results.json");
    let output = run_eval_table(&dir.0, &out);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains("No frames in corpus"), "{stdout}");
    assert!(stdout.starts_with("id "), "{stdout}");
    assert!(stdout.lines().any(|l| l.starts_with("roll")), "{stdout}");
    assert!(
        stdout.contains("Skipped 1 frame(s) and 0 reference render(s)"),
        "{stdout}"
    );
    let v = read_json(&out);
    assert_eq!(v["corpus"]["frames"], 1);
    assert_eq!(v["corpus"]["evaluated"], 0);
    assert_eq!(v["frames"], serde_json::json!([]));
}

#[test]
fn real_corpus_loads_and_is_self_consistent() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus");
    let dir = TempDir::new("real");
    let out = dir.path("eval-results.json");
    let output = run_eval(&corpus, &out);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let v = read_json(&out);
    assert_eq!(v["schema_version"], 2);
    let frames = v["frames"].as_array().expect("frames is an array");
    assert_eq!(
        v["corpus"]["evaluated"].as_u64().unwrap() as usize,
        frames.len()
    );
    let with_references = v["corpus"]["with_references"]
        .as_u64()
        .expect("with_references is a number");
    if with_references == 0 {
        assert!(v["summary"]["de2000_mean"].is_null(), "{}", v["summary"]);
    } else {
        assert!(v["summary"]["de2000_mean"].is_number(), "{}", v["summary"]);
    }
}

#[test]
fn empty_corpus_table_says_no_frames() {
    let dir = TempDir::new("emptytable");
    dir.write("manifest.toml", b"schema_version = 1\n");
    let out = dir.path("eval-results.json");
    let output = run_eval_table(&dir.0, &out);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("No frames in corpus\n"), "{stdout}");
    assert!(out.is_file());
}

#[test]
fn duplicate_id_exits_2_and_names_the_id() {
    let dir = one_frame_corpus("dup");
    let doubled = format!(
        "{MANIFEST}\n{}",
        MANIFEST.trim_start_matches("schema_version = 1\n")
    );
    dir.write("manifest.toml", doubled.as_bytes());
    let out = dir.path("eval-results.json");
    let output = run_eval(&dir.0, &out);
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("r001-01"), "{stderr}");
    assert!(stderr.contains("duplicate"), "{stderr}");
    assert!(!out.exists(), "no results on a manifest error");
}

#[test]
fn unknown_field_exits_2() {
    let dir = one_frame_corpus("unknown");
    dir.write(
        "manifest.toml",
        format!("{MANIFEST}\nexposure = \"n+1\"\n").as_bytes(),
    );
    let out = dir.path("eval-results.json");
    let output = run_eval(&dir.0, &out);
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("exposure"), "{stderr}");
}

#[test]
fn unreadable_reference_exits_1() {
    let dir = one_frame_corpus("badref");
    dir.write("reference/r001-01.negpy.tif", b"definitely not a tiff");
    let out = dir.path("eval-results.json");
    let output = run_eval(&dir.0, &out);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("decode"), "{stderr}");
}

#[test]
fn usage_error_exits_2() {
    let output = Command::new(env!("CARGO_BIN_EXE_negg"))
        .args(["eval", "--no-such-flag"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}

/// The repository's `eval.toml`.
fn repo_eval_toml() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../eval.toml")
}

fn run_eval_with_baseline(corpus: &Path, out: &Path, baseline: &Path, tolerances: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_negg"))
        .args(["eval", "--corpus"])
        .arg(corpus)
        .arg("--out")
        .arg(out)
        .arg("--baseline")
        .arg(baseline)
        .arg("--tolerances")
        .arg(tolerances)
        .output()
        .expect("run negg")
}

#[test]
fn baseline_comparison_passes_against_itself_and_fails_when_doctored() {
    let dir = one_frame_corpus("baseline");
    let first = dir.path("first.json");
    let output = run_eval(&dir.0, &first);
    assert_eq!(output.status.code(), Some(0));

    // Second run compared against the first: the render is deterministic,
    // so nothing can have moved.
    let second = dir.path("second.json");
    let output = run_eval_with_baseline(&dir.0, &second, &first, &repo_eval_toml());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("OK"), "{stdout}");
    assert!(!stdout.contains("REGRESSION"), "{stdout}");
    assert!(stdout.lines().any(|l| l.starts_with("metric")), "{stdout}");
    assert!(stdout.contains("regression gate: OK"), "{stdout}");
    // The results file is unchanged by the comparison.
    assert_eq!(
        std::fs::read_to_string(&first).unwrap(),
        std::fs::read_to_string(&second).unwrap()
    );
    assert!(
        !std::fs::read_to_string(&second)
            .unwrap()
            .contains("baseline")
    );

    // A doctored baseline whose summary de2000_mean is below anything the
    // current run can reach.
    let mut v = read_json(&first);
    v["summary"]["de2000_mean"] = serde_json::json!(-1.0);
    let doctored = dir.path("doctored.json");
    std::fs::write(&doctored, serde_json::to_string_pretty(&v).unwrap()).unwrap();
    let third = dir.path("third.json");
    let output = run_eval_with_baseline(&dir.0, &third, &doctored, &repo_eval_toml());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(output.status.code(), Some(3), "{stdout}");
    assert!(stdout.contains("REGRESSION"), "{stdout}");
    assert!(stdout.contains("regression gate: FAILED"), "{stdout}");
    assert!(third.is_file(), "results are written even on a regression");
}

#[test]
fn roll_variance_rows_compare_their_largest_channel() {
    let dir = one_frame_corpus("rollgate");
    write_negative_tiff(
        &dir.path("frames/r001/r001-02.tif"),
        64,
        48,
        [0.25, 0.55, 0.75],
        [1.6, 1.6, 1.5],
    );
    dir.write(
        "manifest.toml",
        format!("{MANIFEST}{SECOND_FRAME}").as_bytes(),
    );
    let first = dir.path("first.json");
    assert_eq!(run_eval(&dir.0, &first).status.code(), Some(0));
    // Doctor the baseline's roll variance to a tiny value in every
    // channel: the current max channel exceeds it by more than the
    // tolerance.
    let mut v = read_json(&first);
    v["rolls"][0]["dmin_variance"] = serde_json::json!([-1.0, -1.0, -1.0]);
    let doctored = dir.path("doctored.json");
    std::fs::write(&doctored, serde_json::to_string_pretty(&v).unwrap()).unwrap();
    let second = dir.path("second.json");
    let output = run_eval_with_baseline(&dir.0, &second, &doctored, &repo_eval_toml());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(output.status.code(), Some(3), "{stdout}");
    assert!(
        stdout
            .lines()
            .any(|l| l.starts_with("roll r001 dmin_variance") && l.ends_with("REGRESSION")),
        "{stdout}"
    );
    assert!(
        stdout
            .lines()
            .any(|l| l.starts_with("roll r001 gamma_variance") && l.ends_with("OK")),
        "{stdout}"
    );
}

#[test]
fn missing_tracked_case_exits_3() {
    let dir = one_frame_corpus("tracked");
    let first = dir.path("first.json");
    assert_eq!(run_eval(&dir.0, &first).status.code(), Some(0));
    let tolerances = dir.path("eval.toml");
    std::fs::write(
        &tolerances,
        format!(
            "{}\n[[case]]\nid = \"r001-99\"\ntool = \"negpy\"\nmetric = \"de2000_mean\"\ntolerance = 0.5\n",
            std::fs::read_to_string(repo_eval_toml()).unwrap()
        ),
    )
    .unwrap();
    let second = dir.path("second.json");
    let output = run_eval_with_baseline(&dir.0, &second, &first, &tolerances);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(output.status.code(), Some(3), "{stdout}");
    assert!(stdout.contains("tracked case r001-99 missing"), "{stdout}");
}

#[test]
fn tracked_case_on_a_skipped_frame_exits_0_as_skipped() {
    let dir = one_frame_corpus("skippedcase");
    // Baseline produced while the frame and its render were present.
    let first = dir.path("first.json");
    assert_eq!(run_eval(&dir.0, &first).status.code(), Some(0));
    let tolerances = dir.path("eval.toml");
    std::fs::write(
        &tolerances,
        format!(
            "{}\n[[case]]\nid = \"r001-01\"\ntool = \"negpy\"\nmetric = \"de2000_mean\"\ntolerance = 0.5\n\n[[case]]\nid = \"r001-01\"\nmetric = \"neutrality\"\ntolerance = 0.5\n",
            std::fs::read_to_string(repo_eval_toml()).unwrap()
        ),
    )
    .unwrap();

    // The scan is gone from this machine: the case is SKIPPED, not MISSING.
    std::fs::remove_file(dir.path("frames/r001/r001-01.tif")).unwrap();
    let second = dir.path("second.json");
    let output = run_eval_with_baseline(&dir.0, &second, &first, &tolerances);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("SKIPPED"), "{stdout}");
    assert!(!stdout.contains("MISSING"), "{stdout}");
    assert!(stdout.contains("tracked case r001-01 skipped"), "{stdout}");
    assert!(
        stdout.contains("regression gate: OK (2 tracked cases skipped)"),
        "{stdout}"
    );

    // Only the reference gone: the de2000 case is SKIPPED, the neutrality
    // case is measured on the render regardless and passes, and the gate
    // still passes.
    write_frame(&dir.path("frames/r001/r001-01.tif"));
    std::fs::remove_file(dir.path("reference/r001-01.negpy.tif")).unwrap();
    let third = dir.path("third.json");
    let output = run_eval_with_baseline(&dir.0, &third, &first, &tolerances);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(output.status.code(), Some(0), "{stdout}");
    assert!(
        stdout
            .lines()
            .any(|l| l.starts_with("case r001-01 negpy de2000_mean") && l.ends_with("SKIPPED")),
        "{stdout}"
    );
    assert!(
        stdout
            .lines()
            .any(|l| l.starts_with("case r001-01 neutrality") && l.ends_with("OK")),
        "{stdout}"
    );
    assert!(
        stdout.contains("regression gate: OK (1 tracked case skipped)"),
        "{stdout}"
    );
}

#[test]
fn bad_baseline_or_tolerances_exit_2() {
    let dir = one_frame_corpus("badinputs");
    let first = dir.path("first.json");
    assert_eq!(run_eval(&dir.0, &first).status.code(), Some(0));
    let out = dir.path("out.json");

    // Baseline that does not exist.
    let output = run_eval_with_baseline(&dir.0, &out, &dir.path("nope.json"), &repo_eval_toml());
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("baseline"));
    assert!(!out.exists(), "fails before scoring");

    // Baseline with another schema version.
    let doctored = dir.path("schema.json");
    std::fs::write(
        &doctored,
        std::fs::read_to_string(&first)
            .unwrap()
            .replace("\"schema_version\": 2", "\"schema_version\": 3"),
    )
    .unwrap();
    let output = run_eval_with_baseline(&dir.0, &out, &doctored, &repo_eval_toml());
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("schema_version 3"), "{stderr}");

    // Tolerances with an unknown key.
    let tolerances = dir.path("eval.toml");
    std::fs::write(
        &tolerances,
        format!(
            "{}\n[tolerance.extra]\nx = 1\n",
            std::fs::read_to_string(repo_eval_toml()).unwrap()
        ),
    )
    .unwrap();
    let output = run_eval_with_baseline(&dir.0, &out, &first, &tolerances);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("tolerances"));
}

/// A manifest `[[frame]]` entry for the Gate 2 tests: frame `id` of `roll`
/// on film roll `f001` under `light`, with a grey card when `card` and the
/// rig `rig` when given.
fn gate2_entry(id: &str, roll: &str, light: &str, card: bool, rig: Option<&str>) -> String {
    let card = if card {
        "has_grey_card = true\ngrey_card = { x = 0.25, y = 0.25, width = 0.5, height = 0.5 }"
    } else {
        "has_grey_card = false"
    };
    let rig = rig.map_or(String::new(), |r| format!("rig = \"{r}\"\n"));
    format!(
        r#"
[[frame]]
id = "{id}"
file = "frames/{roll}/{id}.tif"
roll = "{roll}"
stock = "Kodak Portra 400"
camera = "Fujifilm X-T4"
light_source = "{light}"
{card}
references = []
notes = ""
contributor = "Test"
licence = "CC0-1.0"
film_roll = "f001"
{rig}"#
    )
}

/// A rig profile with only an identity crosstalk table, so it loads
/// without any measurement frame and leaves the pixels unchanged.
const IDENTITY_RIG: &str = r#"schema_version = 1
name = "test bench"
source = "synthetic identity rig for the eval tests"
measured_on = "2026-10-23"
method = ["crosstalk"]
residual_rms = 0.0

[crosstalk]
method = "crosstalk"
source = "synthetic, no crosstalk"
measured_on = "2026-10-23"
frame_r = "band_r.dng"
frame_g = "band_g.dng"
frame_b = "band_b.dng"
m = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
"#;

#[test]
fn roll_outlier_is_rejected_and_the_rest_are_measured() {
    let dir = TempDir::new("outlier");
    let dmins = [
        [0.20, 0.50, 0.70],
        [0.22, 0.52, 0.71],
        [0.24, 0.51, 0.73],
        // 0.6 log-D above the others in every channel: rejected.
        [0.80, 1.10, 1.30],
    ];
    let mut manifest = String::from("schema_version = 1\n");
    for (i, dmin) in dmins.iter().enumerate() {
        let id = format!("r001-0{}", i + 1);
        write_negative_tiff(
            &dir.path(&format!("frames/r001/{id}.tif")),
            64,
            48,
            *dmin,
            [1.7, 1.7, 1.6],
        );
        manifest.push_str(&gate2_entry(&id, "r001", "Cinestill CS-Lite", false, None));
    }
    dir.write("manifest.toml", manifest.as_bytes());
    let out = dir.path("eval-results.json");
    let output = run_eval(&dir.0, &out);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let v = read_json(&out);
    let roll = &v["rolls"][0];
    assert_eq!(roll["frames"], 4);
    assert_eq!(roll["inliers"], 3, "{roll}");
    assert_eq!(roll["rejected"], serde_json::json!(["r001-04"]), "{roll}");
    assert_eq!(v["frames"][3]["pool"]["dmin_role"], "rejected");
    // The statistics are over the three inliers' own Dmin: small, and well
    // under the spread the outlier would add.
    let std = roll["dmin_std"].as_array().expect("dmin_std");
    for c in 0..3 {
        let s = std[c].as_f64().unwrap();
        assert!(s.is_finite() && s < 0.05, "{std:?}");
        let var = roll["dmin_variance"][c].as_f64().unwrap();
        assert!((var - s * s).abs() < 1e-6, "{roll}");
    }
    // Ruling 59: the rejected frame still renders with the pooled Dmin.
    assert_eq!(v["frames"][3]["estimates"]["dmin"], roll["pooled_dmin"]);
    assert_eq!(v["frames"][3]["estimates"]["dmin_source"], "roll");
    let g = &v["summary"]["gate2"];
    assert!(g["dmin_std_max"].is_number(), "{g}");
    // No film roll qualifies: no verdict.
    assert!(g["three_lights_max_de"].is_null(), "{g}");
    assert!(g["pass"].is_null(), "{g}");
}

#[test]
fn film_roll_under_three_lights_with_a_rig_gives_a_gate2_verdict() {
    let dir = TempDir::new("threelights");
    dir.write("rig/bench.rig.negg.toml", IDENTITY_RIG.as_bytes());
    let rig = Some("rig/bench.rig.negg.toml");
    let mut manifest = String::from("schema_version = 1\n");
    // One scan roll per light; r001 has a second frame so its roll has two
    // Dmin inliers.
    for (id, roll, light, card) in [
        ("r001-01", "r001", "L1", true),
        ("r001-02", "r001", "L1", false),
        ("r002-01", "r002", "L2", true),
        ("r003-01", "r003", "L3", true),
    ] {
        write_negative_tiff(
            &dir.path(&format!("frames/{roll}/{id}.tif")),
            64,
            48,
            [0.2, 0.5, 0.7],
            [1.7, 1.7, 1.6],
        );
        manifest.push_str(&gate2_entry(id, roll, light, card, rig));
    }
    dir.write("manifest.toml", manifest.as_bytes());
    let out = dir.path("eval-results.json");
    let output = run_eval_table(&dir.0, &out);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let v = read_json(&out);
    let t = &v["three_lights"][0];
    assert_eq!(t["film_roll"], "f001");
    assert_eq!(t["qualifies"], true, "{t}");
    assert_eq!(t["lights"].as_array().unwrap().len(), 3, "{t}");
    assert_eq!(v["frames"][0]["rig"], "rig/bench.rig.negg.toml");
    let g = &v["summary"]["gate2"];
    assert!(g["three_lights_max_de"].is_number(), "{g}");
    assert!(g["dmin_std_max"].is_number(), "{g}");
    assert!(g["pass"].is_boolean(), "{g}");
    // Three rolls of one stock, each with a pool: a roll-to-roll entry.
    let r = &v["roll_to_roll"][0];
    assert_eq!(r["stock"], "Kodak Portra 400");
    assert_eq!(r["rolls"], 3);
    assert!(stdout.contains("gate 2: dmin std max "), "{stdout}");
    assert!(stdout.lines().any(|l| l.starts_with("f001")), "{stdout}");

    // Without the rig on roll r003 (its card frame, the last entry) the
    // film roll does not qualify, and Gate 2 has no verdict.
    let rig_line = "rig = \"rig/bench.rig.negg.toml\"\n";
    let at = manifest.rfind(rig_line).unwrap();
    let no_rig = format!("{}{}", &manifest[..at], &manifest[at + rig_line.len()..]);
    dir.write("manifest.toml", no_rig.as_bytes());
    let output = run_eval(&dir.0, &out);
    assert_eq!(output.status.code(), Some(0));
    let v = read_json(&out);
    assert_eq!(v["three_lights"][0]["qualifies"], false);
    assert!(v["summary"]["gate2"]["three_lights_max_de"].is_null());
    assert!(v["summary"]["gate2"]["pass"].is_null());
}

#[test]
fn missing_rig_profile_exits_2_and_names_it() {
    let dir = TempDir::new("norig");
    write_negative_tiff(
        &dir.path("frames/r001/r001-01.tif"),
        64,
        48,
        [0.2, 0.5, 0.7],
        [1.7, 1.7, 1.6],
    );
    let manifest = format!(
        "schema_version = 1\n{}",
        gate2_entry(
            "r001-01",
            "r001",
            "L1",
            true,
            Some("rig/nope.rig.negg.toml")
        )
    );
    dir.write("manifest.toml", manifest.as_bytes());
    let output = run_eval(&dir.0, &dir.path("out.json"));
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("nope.rig.negg.toml"), "{stderr}");
    assert!(stderr.contains("r001"), "{stderr}");
}

#[test]
fn version_1_baseline_is_rejected_with_exit_2() {
    let dir = one_frame_corpus("v1baseline");
    let first = dir.path("first.json");
    assert_eq!(run_eval(&dir.0, &first).status.code(), Some(0));
    // A report as phase 1 wrote it: schema 1, no Gate 2 fields.
    let mut v = read_json(&first);
    v["schema_version"] = serde_json::json!(1);
    let obj = v.as_object_mut().unwrap();
    obj.remove("roll_to_roll");
    obj.remove("three_lights");
    let v1 = dir.path("v1.json");
    std::fs::write(&v1, serde_json::to_string_pretty(&v).unwrap()).unwrap();
    let out = dir.path("out.json");
    let output = run_eval_with_baseline(&dir.0, &out, &v1, &repo_eval_toml());
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("schema_version 1 is not supported (expected 2)"),
        "{stderr}"
    );
    assert!(stderr.contains("regenerate the baseline"), "{stderr}");
    assert!(!out.exists(), "fails before scoring");
}
