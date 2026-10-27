// SPDX-License-Identifier: Apache-2.0

//! End-to-end tests for tone presets on the command line (ADR 0034):
//! `--preset` on `negg convert` and `negg batch`, `--preset-all-frames` and
//! `negg preset list`. Every spawned `negg` gets its own `NEGG_PRESETS_DIR`,
//! so the user's own presets folder is never read.

mod common;

use std::path::Path;
use std::process::{Command, Output};

use common::{TempDir, read_json, write_negative_tiff};
use negg_types::{Preset, PrintLook, Settings, builtin_presets};

/// Runs `negg` with `NEGG_PRESETS_DIR` set to `presets`.
fn negg_in(presets: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_negg"))
        .env("NEGG_PRESETS_DIR", presets)
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

fn ok(o: &Output) {
    assert_eq!(o.status.code(), Some(0), "{}\n{}", stdout(o), stderr(o));
}

fn rich() -> Preset {
    builtin_presets()
        .into_iter()
        .find(|p| p.name == "Rich")
        .unwrap()
}

/// The settings `Settings::default()` with Rich's look and label.
fn default_plus_rich() -> Settings {
    let mut s = Settings::default();
    rich().look.apply_to_settings("Rich", &mut s.print);
    s
}

fn sidecar(path: &Path) -> negg_io::Sidecar {
    negg_io::read_sidecar(path).unwrap()
}

fn write_roll(dir: &TempDir) {
    for (name, dmin) in [
        ("a.tif", [0.20, 0.50, 0.70]),
        ("b.tif", [0.21, 0.51, 0.71]),
        ("c.tif", [0.22, 0.52, 0.72]),
    ] {
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
fn convert_with_a_preset_records_its_look_and_label() {
    let dir = TempDir::new("preset-convert");
    let presets = dir.path("presets");
    let input = dir.path("in/f.tif");
    write_negative_tiff(&input, 64, 48, [0.2, 0.5, 0.7], [1.6, 1.7, 1.8]);
    // A preset name is case-insensitive.
    ok(&negg_in(
        &presets,
        &["convert", input.to_str().unwrap(), "--preset", "rich"],
    ));
    let s = sidecar(&dir.path("in/f.negg.json"));
    assert_eq!(s.settings, default_plus_rich());
    assert_eq!(s.settings.print.preset.as_deref(), Some("Rich"));
    assert!(!s.overrides.print.has_own_look());

    // `--preset` applies after `--settings`: the look is the preset's, the
    // rest is the settings file's.
    let mut file = Settings::default();
    file.print.grade = 70.0;
    file.print.exposure = 0.25;
    dir.write("s.json", serde_json::to_string(&file).unwrap().as_bytes());
    ok(&negg_in(
        &presets,
        &[
            "convert",
            input.to_str().unwrap(),
            "--settings",
            dir.path("s.json").to_str().unwrap(),
            "--preset",
            "Rich",
        ],
    ));
    let s = sidecar(&dir.path("in/f.negg.json"));
    let mut want = file;
    rich().look.apply_to_settings("Rich", &mut want.print);
    assert_eq!(s.settings, want);

    // A preset file by path.
    let custom = Preset {
        name: "Mine".into(),
        look: PrintLook {
            grade: 100.0,
            ..rich().look
        },
    };
    let path = dir.path("mine.preset.negg.toml");
    negg_io::write_preset(&path, &custom).unwrap();
    ok(&negg_in(
        &presets,
        &[
            "convert",
            input.to_str().unwrap(),
            "--preset",
            path.to_str().unwrap(),
        ],
    ));
    let s = sidecar(&dir.path("in/f.negg.json"));
    assert_eq!(s.settings.print.grade, 100.0);
    assert_eq!(s.settings.print.preset.as_deref(), Some("Mine"));
}

#[test]
fn convert_in_a_roll_puts_the_preset_in_the_frame_overrides() {
    let dir = TempDir::new("preset-convert-roll");
    let presets = dir.path("presets");
    write_roll(&dir);
    let roll = dir.path("roll");
    ok(&negg_in(&presets, &["batch", roll.to_str().unwrap()]));
    let roll_before = std::fs::read(roll.join("roll.negg.json")).unwrap();
    let o = negg_in(
        &presets,
        &[
            "convert",
            roll.join("b.tif").to_str().unwrap(),
            "--preset",
            "Rich",
        ],
    );
    ok(&o);
    assert!(stderr(&o).contains("using the roll settings and pool"));
    let s = sidecar(&roll.join("b.negg.json"));
    let mut want = negg_types::roll::PrintOverrides::default();
    rich().look.apply_to_overrides("Rich", &mut want);
    assert_eq!(s.overrides.print, want);
    assert_eq!(s.settings.print, default_plus_rich().print);
    assert_eq!(s.roll.as_deref(), Some("roll.negg.json"));
    // `negg convert` never writes the roll sidecar.
    assert!(std::fs::read(roll.join("roll.negg.json")).unwrap() == roll_before);
}

#[test]
fn batch_preset_keeps_frame_looks_unless_all_frames() {
    let dir = TempDir::new("preset-batch");
    let presets = dir.path("presets");
    write_roll(&dir);
    let roll = dir.path("roll");
    let roll_arg = roll.to_str().unwrap();
    ok(&negg_in(&presets, &["batch", roll_arg]));
    // Give frame b its own grade.
    let b_path = roll.join("b.negg.json");
    let mut b = read_json(&b_path);
    b["overrides"] = serde_json::json!({ "print": { "grade": 100.0 } });
    std::fs::write(&b_path, serde_json::to_string_pretty(&b).unwrap()).unwrap();

    ok(&negg_in(&presets, &["batch", roll_arg, "--preset", "Rich"]));
    let stored = negg_io::read_roll_sidecar(&roll.join("roll.negg.json")).unwrap();
    assert_eq!(stored.settings.shared.print, default_plus_rich().print);
    for f in ["a", "c"] {
        let s = sidecar(&roll.join(format!("{f}.negg.json")));
        assert_eq!(s.settings.print, default_plus_rich().print, "{f}");
        assert!(!s.overrides.print.has_own_look(), "{f}");
    }
    // b keeps the whole look it rendered with before (Ruling 5).
    let s = sidecar(&b_path);
    assert_eq!(s.overrides.print.grade, Some(100.0));
    let old = Settings::default().print;
    assert_eq!(s.overrides.print.toe, Some(old.toe));
    assert_eq!(s.overrides.print.shoulder, Some(old.shoulder));
    assert_eq!(
        s.overrides.print.black_point_compensation,
        Some(old.black_point_compensation)
    );
    assert_eq!(s.overrides.print.saturation, Some(old.saturation));
    // Its resolved print is exactly the old one, the label included: it
    // had none, pinned as "" (Ruling 8).
    assert_eq!(s.overrides.print.preset.as_deref(), Some(""));
    assert_eq!(s.settings.print.preset, None);
    assert_eq!(s.settings.print, old_with_grade(100.0));

    // With --preset-all-frames b's look fields are cleared, so it takes
    // the roll's.
    ok(&negg_in(
        &presets,
        &["batch", roll_arg, "--preset", "Rich", "--preset-all-frames"],
    ));
    let s = sidecar(&b_path);
    assert!(!s.overrides.print.has_own_look(), "{:?}", s.overrides);
    assert_eq!(s.settings.print, default_plus_rich().print);
}

fn old_with_grade(grade: f32) -> negg_types::settings::PrintSettings {
    let mut p = Settings::default().print;
    p.grade = grade;
    p
}

#[test]
fn preset_list_shows_builtins_and_user_presets() {
    let dir = TempDir::new("preset-list");
    let presets = dir.path("presets");
    std::fs::create_dir_all(&presets).unwrap();
    let mine = presets.join("my-look.preset.negg.toml");
    negg_io::write_preset(
        &mine,
        &Preset {
            name: "My Look".into(),
            look: rich().look,
        },
    )
    .unwrap();
    std::fs::write(presets.join("broken.preset.negg.toml"), "name = [").unwrap();
    let o = negg_in(&presets, &["preset", "list"]);
    ok(&o);
    let out = stdout(&o);
    let lines: Vec<&str> = out.lines().collect();
    let builtins = builtin_presets();
    assert_eq!(lines.len(), builtins.len() + 1, "{out}");
    for (line, p) in lines.iter().zip(&builtins) {
        assert_eq!(*line, format!("{}\t(built-in)", p.name));
    }
    assert_eq!(
        *lines.last().unwrap(),
        format!("My Look\t{}", mine.display())
    );
    let err = stderr(&o);
    assert!(
        err.starts_with("negg preset: warning: ") && err.contains("broken.preset.negg.toml"),
        "{err}"
    );
}

#[test]
fn unknown_preset_is_a_usage_error() {
    let dir = TempDir::new("preset-unknown");
    let presets = dir.path("presets");
    write_roll(&dir);
    let roll = dir.path("roll");
    let o = negg_in(
        &presets,
        &[
            "convert",
            roll.join("a.tif").to_str().unwrap(),
            "--preset",
            "Nonesuch",
        ],
    );
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
    let err = stderr(&o);
    assert!(err.starts_with("negg convert: "), "{err}");
    for name in ["Nonesuch", "Standard", "Rich", "Linear"] {
        assert!(err.contains(name), "{err}");
    }
    assert!(!err.contains("skipped"), "{err}");
    // A user file of that name that is skipped: the error says which file
    // and why, not only that the name is unknown.
    std::fs::create_dir_all(&presets).unwrap();
    let hot = presets.join("hot.preset.negg.toml");
    std::fs::write(
        &hot,
        "schema_version = 1\nname = \"Hot\"\ngrade = 300.0\ntoe = 6.0\nshoulder = 3.0\n\
         black_point_compensation = false\nsaturation = 1.0\n",
    )
    .unwrap();
    let o = negg_in(
        &presets,
        &[
            "convert",
            roll.join("a.tif").to_str().unwrap(),
            "--preset",
            "Hot",
        ],
    );
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
    let err = stderr(&o);
    assert!(err.contains("no preset named \"Hot\""), "{err}");
    assert_eq!(err.matches(hot.to_str().unwrap()).count(), 1, "{err}");
    assert!(err.contains("skipped") && err.contains("grade"), "{err}");
    std::fs::remove_file(&hot).unwrap();
    let o = negg_in(
        &presets,
        &["batch", roll.to_str().unwrap(), "--preset", "Nonesuch"],
    );
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
    assert!(stderr(&o).starts_with("negg batch: "), "{}", stderr(&o));
    let o = negg_in(
        &presets,
        &[
            "batch",
            roll.to_str().unwrap(),
            "--preset",
            dir.path("missing.preset.negg.toml").to_str().unwrap(),
        ],
    );
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
    assert!(stderr(&o).contains("missing.preset.negg.toml"));
    // Nothing was written.
    assert!(!roll.join("roll.negg.json").exists());
    assert!(!roll.join("a.negg.json").exists());
}

#[test]
fn preset_all_frames_without_roll_mode_is_an_error() {
    let dir = TempDir::new("preset-all-no-roll");
    let presets = dir.path("presets");
    write_roll(&dir);
    let roll = dir.path("roll");
    let roll_arg = roll.to_str().unwrap();
    let frame = roll.join("a.tif");
    for args in [
        vec![
            "batch",
            roll_arg,
            "--no-roll",
            "--preset",
            "Rich",
            "--preset-all-frames",
        ],
        vec![
            "convert",
            frame.to_str().unwrap(),
            "--preset",
            "Rich",
            "--preset-all-frames",
        ],
        // It needs --preset.
        vec!["batch", roll_arg, "--preset-all-frames"],
    ] {
        let o = negg_in(&presets, &args);
        assert_eq!(o.status.code(), Some(2), "{args:?}: {}", stderr(&o));
        assert!(stderr(&o).contains("--preset"), "{args:?}: {}", stderr(&o));
        // Refused for what it means, not as an unknown flag.
        assert!(
            !stderr(&o).contains("unexpected argument"),
            "{args:?}: {}",
            stderr(&o)
        );
    }
    // Nothing was written.
    let written: Vec<_> = std::fs::read_dir(&roll)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .filter(|n| n.to_string_lossy().contains(".negg."))
        .collect();
    assert!(written.is_empty(), "{written:?}");
}

#[test]
fn no_roll_preset_warns_when_a_frame_keeps_its_own_look() {
    let dir = TempDir::new("preset-no-roll-own");
    let presets = dir.path("presets");
    write_roll(&dir);
    let roll = dir.path("roll");
    let roll_arg = roll.to_str().unwrap();
    ok(&negg_in(&presets, &["batch", roll_arg, "--no-roll"]));
    let b_path = roll.join("b.negg.json");
    let mut b = read_json(&b_path);
    b["overrides"] = serde_json::json!({ "print": { "grade": 100.0 } });
    std::fs::write(&b_path, serde_json::to_string_pretty(&b).unwrap()).unwrap();

    let o = negg_in(
        &presets,
        &["batch", roll_arg, "--no-roll", "--preset", "Rich"],
    );
    ok(&o);
    let err = stderr(&o);
    let warns: Vec<&str> = err
        .lines()
        .filter(|l| l.contains("wins over --preset"))
        .collect();
    assert_eq!(warns.len(), 1, "{err}");
    assert!(
        warns[0].starts_with("negg batch: warning: b.tif: "),
        "{err}"
    );
    // b keeps its grade; the others take the preset.
    assert_eq!(sidecar(&b_path).settings.print.grade, 100.0);
    assert_eq!(
        sidecar(&roll.join("a.negg.json")).settings.print,
        default_plus_rich().print
    );

    let o = negg_in(
        &presets,
        &[
            "convert",
            roll.join("b.tif").to_str().unwrap(),
            "--no-roll",
            "--preset",
            "Rich",
        ],
    );
    ok(&o);
    assert!(
        stderr(&o).contains("negg convert: warning: b.tif: its sidecar overrides set its own look"),
        "{}",
        stderr(&o)
    );
    // No warning for a frame without its own look.
    let o = negg_in(
        &presets,
        &[
            "convert",
            roll.join("a.tif").to_str().unwrap(),
            "--no-roll",
            "--preset",
            "Rich",
        ],
    );
    ok(&o);
    assert!(!stderr(&o).contains("wins over"), "{}", stderr(&o));
}
