// SPDX-License-Identifier: MPL-2.0

//! Settings schema migrations (Ruling 71): hand-written phase 1 and phase 2
//! fixtures migrate to the current schema with identity geometry and every
//! other field unchanged, round-trip through a file, and newer versions
//! are rejected.

use std::path::{Path, PathBuf};

use negg_io::roll::migrate_roll;
use negg_io::{
    IoError, ROLL_SIDECAR_SCHEMA_VERSION, SIDECAR_SCHEMA_VERSION, migrate, read_roll_sidecar,
    read_sidecar, write_roll_sidecar, write_sidecar,
};
use negg_types::settings::{ChannelEstimate, OutputSpace, SETTINGS_SCHEMA_VERSION};
use negg_types::{Geometry, Settings};
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
        let dir = std::env::temp_dir().join(format!(
            "negg-io-migrations-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
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
    for name in ["sidecar-v1.json", "sidecar-v2.json"] {
        let s = migrate(fixture(name)).unwrap();
        let path = dir.0.join("frame.negg.json");
        write_sidecar(&path, &s).unwrap();
        assert_eq!(read_sidecar(&path).unwrap(), s, "{name}");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"schema_version\": 3"), "{text}");
        assert!(text.contains("\"geometry\""), "{text}");
    }
}

#[test]
fn roll_v1_migrates_to_current() {
    let r = migrate_roll(fixture("roll-v1.json")).unwrap();
    assert_eq!(r.schema_version, ROLL_SIDECAR_SCHEMA_VERSION);
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
        migrate(json!({ "schema_version": 4 })),
        Err(IoError::Schema(_))
    ));
    assert!(matches!(
        migrate_roll(json!({ "schema_version": 3 })),
        Err(IoError::Schema(_))
    ));
}
