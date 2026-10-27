// SPDX-License-Identifier: MPL-2.0

//! Settings schema migrations (Rulings 71 and 134): hand-written phase 1
//! and phase 2 fixtures migrate to the current schema with identity
//! geometry, phase 3 (schema 3) fixtures keep their geometry with no
//! analysis area, every other field unchanged; they round-trip through a
//! file, and newer versions are rejected. Schema 4 files (and roll schema 3)
//! migrate to schema 5 unchanged apart from the versions (ADR 0033).

use std::path::{Path, PathBuf};

use negg_io::roll::migrate_roll;
use negg_io::{
    IoError, ROLL_SIDECAR_SCHEMA_VERSION, SIDECAR_SCHEMA_VERSION, migrate, read_roll_sidecar,
    read_sidecar, write_roll_sidecar, write_sidecar,
};
use negg_types::settings::{ChannelEstimate, OutputSpace, SETTINGS_SCHEMA_VERSION};
use negg_types::{CropRect, Geometry, Settings};
use serde_json::{Value, json};

fn fixture(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/migrations")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> TempDir {
        // The counter keeps names unique when tests in one process start
        // within the clock's resolution (microseconds on macOS).
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "negg-io-migrations-{tag}-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn sidecar_v1_migrates_to_current_with_identity_geometry() {
    let s = migrate(fixture("sidecar-v1.json")).unwrap();
    assert_eq!(s.schema_version, SIDECAR_SCHEMA_VERSION);
    assert_eq!(s.settings.schema_version, SETTINGS_SCHEMA_VERSION);
    assert_eq!(s.settings.geometry, Geometry::default());
    let mut want = Settings::default();
    want.print.grade = 100.0;
    want.print.exposure = 0.35;
    want.print.cmy = [0.0, 0.01, -0.02];
    assert_eq!(s.settings, want);
    assert_eq!(s.source, "frame.dng");
    let e = s.estimates.unwrap();
    assert_eq!(e.dmin, [0.21, 0.55, 0.78]);
    assert_eq!(e.neutral_offset, [0.0; 3]);
    assert!(s.overrides.is_empty());
}

#[test]
fn sidecar_v2_migrates_and_keeps_its_overrides() {
    let s = migrate(fixture("sidecar-v2.json")).unwrap();
    assert_eq!(s.schema_version, SIDECAR_SCHEMA_VERSION);
    assert_eq!(s.settings.schema_version, SETTINGS_SCHEMA_VERSION);
    assert_eq!(s.settings.geometry, Geometry::default());
    assert_eq!(s.settings.colour.output_space, OutputSpace::AdobeRgb);
    assert_eq!(
        s.settings.film.gamma,
        ChannelEstimate::Fixed { value: [1.6; 3] }
    );
    assert_eq!(
        s.settings.profiles.rig.as_deref(),
        Some("/rigs/bench.rig.negg.toml")
    );
    assert!(s.settings.print.black_point_compensation);
    assert_eq!(s.roll.as_deref(), Some("roll.negg.json"));
    assert_eq!(
        s.overrides.film.gamma,
        Some(ChannelEstimate::Fixed { value: [1.6; 3] })
    );
    assert_eq!(s.overrides.print.exposure, Some(0.35));
    assert_eq!(s.overrides.print.black_point_compensation, Some(true));
    assert_eq!(s.overrides.geometry, None);
}

#[test]
fn a_migrated_sidecar_round_trips_through_a_file() {
    let dir = TempDir::new("sidecar");
    for name in [
        "sidecar-v1.json",
        "sidecar-v2.json",
        "sidecar-v3.json",
        "sidecar-v4.json",
    ] {
        let s = migrate(fixture(name)).unwrap();
        let path = dir.0.join("frame.negg.json");
        write_sidecar(&path, &s).unwrap();
        assert_eq!(read_sidecar(&path).unwrap(), s, "{name}");
        let text = std::fs::read_to_string(&path).unwrap();
        let version = format!("\"schema_version\": {SIDECAR_SCHEMA_VERSION}");
        assert!(text.contains(&version), "{text}");
        assert!(text.contains("\"geometry\""), "{text}");
    }
}

#[test]
fn roll_v1_migrates_to_current() {
    let r = migrate_roll(fixture("roll-v1.json")).unwrap();
    assert_eq!(r.schema_version, ROLL_SIDECAR_SCHEMA_VERSION);
    assert_eq!(r.settings.schema_version, SETTINGS_SCHEMA_VERSION);
    assert_eq!(r.settings.shared.schema_version, SETTINGS_SCHEMA_VERSION);
    assert_eq!(r.settings.shared.geometry, Geometry::default());
    assert_eq!(r.settings.shared.print.grade, 110.0);
    assert_eq!(r.settings.shared.print.exposure, 0.1);
    assert!(!r.settings.pool_gamma);
    assert_eq!(r.frames, ["a.dng", "b.dng"]);
    let pool = r.pool.as_ref().unwrap();
    assert_eq!(pool.dmin, Some([0.25, 0.5, 0.75]));
    assert_eq!(pool.frames.len(), 1);

    let dir = TempDir::new("roll");
    let path = dir.0.join("roll.negg.json");
    write_roll_sidecar(&path, &r).unwrap();
    assert_eq!(read_roll_sidecar(&path).unwrap(), r);
    // Reading the v1 file itself goes through the same ladder.
    std::fs::write(&path, fixture("roll-v1.json").to_string()).unwrap();
    assert_eq!(read_roll_sidecar(&path).unwrap(), r);
}

#[test]
fn sidecar_v3_migrates_to_v4_with_no_analysis_area() {
    let s = migrate(fixture("sidecar-v3.json")).unwrap();
    assert_eq!(s.schema_version, SIDECAR_SCHEMA_VERSION);
    assert_eq!(SIDECAR_SCHEMA_VERSION, 5);
    assert_eq!(s.settings.schema_version, SETTINGS_SCHEMA_VERSION);
    assert_eq!(SETTINGS_SCHEMA_VERSION, 5);
    let crop = CropRect {
        x: 0.05,
        y: 0.04,
        width: 0.9,
        height: 0.92,
    };
    let want = Geometry {
        crop: Some(crop),
        analysis_area: None,
        analysis_buffer: 0.03,
        rotate_degrees: 90,
        flip_horizontal: false,
    };
    assert_eq!(s.settings.geometry, want);
    assert_eq!(s.overrides.geometry, Some(want));
    assert_eq!(s.settings.print.grade, 115.0);
    assert_eq!(s.overrides.print.exposure, Some(0.2));
    assert_eq!(s.estimates.unwrap().gamma, [1.69, 1.65, 1.62]);
    // Written back, the file is version 5 with no analysis_area key.
    let dir = TempDir::new("sidecar-v3");
    let path = dir.0.join("frame.negg.json");
    write_sidecar(&path, &s).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"schema_version\": 5"), "{text}");
    assert!(!text.contains("analysis_area"), "{text}");
    assert_eq!(read_sidecar(&path).unwrap(), s);
}

#[test]
fn roll_v2_migrates_to_v3_with_no_analysis_area_and_no_flag() {
    let r = migrate_roll(fixture("roll-v2.json")).unwrap();
    assert_eq!(r.schema_version, ROLL_SIDECAR_SCHEMA_VERSION);
    assert_eq!(ROLL_SIDECAR_SCHEMA_VERSION, 4);
    assert_eq!(r.settings.schema_version, SETTINGS_SCHEMA_VERSION);
    assert_eq!(r.settings.shared.schema_version, SETTINGS_SCHEMA_VERSION);
    let g = r.settings.shared.geometry;
    assert!(g.crop.is_some());
    assert_eq!(g.analysis_area, None);
    assert_eq!(g.analysis_rect(), g.crop);
    let pool = r.pool.as_ref().unwrap();
    assert_eq!(pool.gamma, Some([1.6, 1.7, 1.8]));
    assert!(!pool.frames[0].needs_analysis_area);

    let dir = TempDir::new("roll-v2");
    let path = dir.0.join("roll.negg.json");
    write_roll_sidecar(&path, &r).unwrap();
    assert_eq!(read_roll_sidecar(&path).unwrap(), r);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\n  \"schema_version\": 4"), "{text}");
    assert!(!text.contains("analysis_area"), "{text}");
    assert!(!text.contains("needs_analysis_area"), "{text}");
    std::fs::write(&path, fixture("roll-v2.json").to_string()).unwrap();
    assert_eq!(read_roll_sidecar(&path).unwrap(), r);
}

#[test]
fn settings_v2_loads_with_identity_geometry() {
    let s: Settings = serde_json::from_value(fixture("settings-v2.json")).unwrap();
    assert_eq!(s.geometry, Geometry::default());
    let mut want = Settings {
        schema_version: 2,
        ..Settings::default()
    };
    want.film.film_base = ChannelEstimate::Fixed {
        value: [0.2, 0.5, 0.7],
    };
    want.print.grade = 120.0;
    want.print.exposure = -0.2;
    want.colour.auto_colour = false;
    want.colour.output_space = OutputSpace::DisplayP3;
    assert_eq!(s, want);
    s.validate().unwrap();
}

#[test]
fn newer_versions_are_rejected() {
    assert!(matches!(
        migrate(json!({ "schema_version": 6 })),
        Err(IoError::Schema(_))
    ));
    assert!(matches!(
        migrate_roll(json!({ "schema_version": 5 })),
        Err(IoError::Schema(_))
    ));
}

#[test]
fn schema_4_files_migrate_to_5_unchanged() {
    let s = migrate(fixture("sidecar-v4.json")).unwrap();
    assert_eq!(s.schema_version, 5);
    assert_eq!(s.settings.schema_version, 5);
    assert_eq!(s.settings.print.saturation, 1.0);
    assert_eq!(s.settings.print.preset, None);
    assert_eq!(s.overrides.print.saturation, None);
    assert_eq!(s.overrides.print.preset, None);
    // Every other field equals the parsed old value.
    let old: Settings =
        serde_json::from_value(fixture("sidecar-v4.json")["settings"].clone()).unwrap();
    assert_eq!(
        s.settings,
        Settings {
            schema_version: 5,
            ..old
        }
    );
    assert!(s.settings.geometry.analysis_area.is_some());
    assert_eq!(s.overrides.print.exposure, Some(0.2));
    assert_eq!(s.estimates.unwrap().x0, 1.25);

    let r = migrate_roll(fixture("roll-v3.json")).unwrap();
    assert_eq!(r.schema_version, 4);
    assert_eq!(r.settings.schema_version, 5);
    assert_eq!(r.settings.shared.schema_version, 5);
    assert_eq!(r.settings.shared.print.saturation, 1.0);
    assert_eq!(r.settings.shared.print.preset, None);
    assert!(r.settings.shared.geometry.analysis_area.is_some());
    assert!(r.pool.as_ref().unwrap().frames[0].needs_analysis_area);
    assert_eq!(r.settings.shared.print.grade, 115.0);

    // Written back, neither new key appears.
    let dir = TempDir::new("schema5");
    let path = dir.0.join("frame.negg.json");
    write_sidecar(&path, &s).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        !text.contains("\"saturation\"") && !text.contains("\"preset\""),
        "{text}"
    );
    assert_eq!(read_sidecar(&path).unwrap(), s);
    let path = dir.0.join("roll.negg.json");
    write_roll_sidecar(&path, &r).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        !text.contains("\"saturation\"") && !text.contains("\"preset\""),
        "{text}"
    );
    assert_eq!(read_roll_sidecar(&path).unwrap(), r);
}
