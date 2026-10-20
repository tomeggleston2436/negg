// SPDX-License-Identifier: Apache-2.0

//! End-to-end tests for `negg eval` through the built binary.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tiff::encoder::TiffEncoder;
use tiff::encoder::colortype::RGB16;

/// A temp directory under `std::env::temp_dir()`, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> TempDir {
        let dir = std::env::temp_dir().join(format!(
            "negg-eval-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }

    fn write(&self, rel: &str, bytes: &[u8]) {
        let p = self.path(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Writes a 16x16 16-bit RGB TIFF of uniform mid grey.
fn write_grey_tiff(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let data = vec![30_000u16; 16 * 16 * 3];
    let mut enc = TiffEncoder::new(File::create(path).unwrap()).unwrap();
    enc.write_image::<RGB16>(16, 16, &data).unwrap();
}

const MANIFEST: &str = r#"schema_version = 1

[[frame]]
id = "r001-01"
file = "frames/r001/r001-01.dng"
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

/// A corpus with one frame, one grey card and one reference render.
fn one_frame_corpus(tag: &str) -> TempDir {
    let dir = TempDir::new(tag);
    dir.write("frames/r001/r001-01.dng", b"not a real raw");
    write_grey_tiff(&dir.path("reference/r001-01.negpy.tif"));
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

#[test]
fn one_frame_scores_zero_against_itself() {
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
    assert_eq!(v["schema_version"], 1);
    assert_eq!(v["corpus"]["frames"], 1);
    assert_eq!(v["corpus"]["with_references"], 1);
    assert_eq!(v["corpus"]["with_grey_card"], 1);
    let f = &v["frames"][0];
    assert_eq!(f["id"], "r001-01");
    assert_eq!(f["references"][0]["tool"], "negpy");
    assert_eq!(f["references"][0]["de2000"]["mean"], 0.0);
    assert_eq!(f["references"][0]["de2000"]["p95"], 0.0);
    assert_eq!(f["references"][0]["de2000"]["max"], 0.0);
    assert!(f["neutrality"].is_object(), "{f}");
    assert!(f["neutrality"]["a_mean_abs"].as_f64().unwrap().abs() < 1e-3);
    assert!(f["estimates"].is_null());
    assert_eq!(v["rolls"].as_array().unwrap().len(), 1);
    assert_eq!(v["rolls"][0]["roll"], "r001");
    assert_eq!(v["rolls"][0]["frames"], 1);
    assert!(v["rolls"][0]["dmin_variance"].is_null());
    assert!(v["rolls"][0]["gamma_variance"].is_null());
    assert_eq!(v["summary"]["de2000_mean"], 0.0);
    assert_eq!(v["summary"]["de2000_p95_mean"], 0.0);
    assert!(v["summary"]["neutrality_mean"].is_number());
    // Diffable: nothing in the file names the temp directory.
    assert!(!text.contains(dir.0.to_str().unwrap()));
}

#[test]
fn one_frame_prints_a_table_when_not_quiet() {
    let dir = one_frame_corpus("table");
    let out = dir.path("eval-results.json");
    let output = Command::new(env!("CARGO_BIN_EXE_negg"))
        .args(["eval", "--corpus"])
        .arg(&dir.0)
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("r001-01"), "{stdout}");
    assert!(stdout.contains("negpy"), "{stdout}");
    assert!(stdout.lines().any(|l| l.starts_with("roll")), "{stdout}");
    assert!(
        stdout.lines().last().unwrap().starts_with("summary:"),
        "{stdout}"
    );
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
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(v["schema_version"], 1);
    let frames = v["frames"].as_array().expect("frames is an array");
    assert_eq!(v["corpus"]["frames"], frames.len());
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
    let output = Command::new(env!("CARGO_BIN_EXE_negg"))
        .args(["eval", "--corpus"])
        .arg(&dir.0)
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
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

    // Second run compared against the first: nothing can have moved.
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
    let mut v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&first).unwrap()).unwrap();
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
            .replace("\"schema_version\": 1", "\"schema_version\": 2"),
    )
    .unwrap();
    let output = run_eval_with_baseline(&dir.0, &out, &doctored, &repo_eval_toml());
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("schema_version 2"), "{stderr}");

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
