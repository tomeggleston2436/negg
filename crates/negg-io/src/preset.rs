// SPDX-License-Identifier: MPL-2.0

//! TOML reading and writing for tone preset files (ADR 0034).
//!
//! The types and validation live in negg-types; this module only moves a
//! [`Preset`] to and from a `*.preset.negg.toml` file. Both directions
//! validate. No maths is done here.

use std::fs;
use std::path::Path;

use negg_types::{Preset, PrintLook, validate_preset_name};
use serde::{Deserialize, Serialize};

use crate::error::{IoError, io_at};
use crate::sidecar::write_atomic;

/// The file name suffix of a preset file.
pub const PRESET_SUFFIX: &str = ".preset.negg.toml";

/// The preset file schema this build reads and writes.
const PRESET_SCHEMA_VERSION: u32 = 1;

/// The on-disk shape: the name and the five look keys, flat.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PresetFile {
    schema_version: u32,
    name: String,
    grade: f32,
    toe: f32,
    shoulder: f32,
    black_point_compensation: bool,
    saturation: f32,
}

/// Why `preset` is invalid, as the bare reason; each caller wraps it in an
/// [`IoError`] once.
fn check(preset: &Preset) -> Result<(), String> {
    validate_preset_name(&preset.name).map_err(|e| e.to_string())?;
    preset.look.validate().map_err(|e| e.to_string())
}

/// Reads and validates a preset file.
///
/// # Errors
///
/// [`IoError::Io`] for file errors and [`IoError::Profile`] for TOML errors
/// (including unknown fields), a newer schema and validation failures.
pub fn read_preset(path: &Path) -> Result<Preset, IoError> {
    let fail = |why: String| IoError::Profile(format!("{}: {why}", path.display()));
    let text = fs::read_to_string(path).map_err(io_at(path))?;
    let value: toml::Value = toml::from_str(&text).map_err(|e| fail(e.to_string()))?;
    match value
        .get("schema_version")
        .and_then(toml::Value::as_integer)
    {
        Some(v) if v > i64::from(PRESET_SCHEMA_VERSION) => {
            return Err(fail(format!(
                "preset schema_version {v} is newer than this build understands (up to \
                 {PRESET_SCHEMA_VERSION}); upgrade negg to read it"
            )));
        }
        Some(v) if v == i64::from(PRESET_SCHEMA_VERSION) => {}
        _ => {
            return Err(fail(format!(
                "preset schema_version must be {PRESET_SCHEMA_VERSION}"
            )));
        }
    }
    let file: PresetFile = toml::from_str(&text).map_err(|e| fail(e.to_string()))?;
    let preset = Preset {
        name: file.name,
        look: PrintLook {
            grade: file.grade,
            toe: file.toe,
            shoulder: file.shoulder,
            black_point_compensation: file.black_point_compensation,
            saturation: file.saturation,
        },
    };
    check(&preset).map_err(fail)?;
    Ok(preset)
}

/// Validates then writes a preset file, atomically.
///
/// # Errors
///
/// [`IoError::Profile`] when validation or serialisation fails,
/// [`IoError::Io`] for file errors.
pub fn write_preset(path: &Path, preset: &Preset) -> Result<(), IoError> {
    check(preset).map_err(IoError::Profile)?;
    let file = PresetFile {
        schema_version: PRESET_SCHEMA_VERSION,
        name: preset.name.clone(),
        grade: preset.look.grade,
        toe: preset.look.toe,
        shoulder: preset.look.shoulder,
        black_point_compensation: preset.look.black_point_compensation,
        saturation: preset.look.saturation,
    };
    let text = toml::to_string_pretty(&file).map_err(|e| IoError::Profile(e.to_string()))?;
    write_atomic(path, text.as_bytes()).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "negg-io-preset-{name}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn mine() -> Preset {
        Preset {
            name: "My warm".into(),
            look: PrintLook {
                grade: 105.0,
                toe: 5.5,
                shoulder: 2.5,
                black_point_compensation: true,
                saturation: 1.15,
            },
        }
    }

    const BODY: &str = "name = \"x\"\ngrade = 115.0\ntoe = 6.0\nshoulder = 3.0\n\
                        black_point_compensation = false\nsaturation = 1.0\n";

    #[test]
    fn preset_round_trips_through_toml() {
        let dir = TempDir::new("rt");
        let path = dir.path(&format!("warm{PRESET_SUFFIX}"));
        write_preset(&path, &mine()).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("schema_version = 1"));
        assert_eq!(read_preset(&path).unwrap(), mine());
    }

    #[test]
    fn preset_with_out_of_range_grade_is_rejected() {
        let dir = TempDir::new("grade");
        let path = dir.path("bad.preset.negg.toml");
        fs::write(
            &path,
            format!("schema_version = 1\n{}", BODY.replace("115.0", "200.0")),
        )
        .unwrap();
        let err = read_preset(&path).unwrap_err().to_string();
        assert!(err.contains("grade"), "{err}");
        // Wrapped once: the path and the error kind each appear once.
        let shown = path.display().to_string();
        assert_eq!(err.matches(shown.as_str()).count(), 1, "{err}");
        assert_eq!(err.matches("profile error:").count(), 1, "{err}");

        let mut bad = mine();
        bad.look.grade = 200.0;
        assert!(write_preset(&dir.path("w.preset.negg.toml"), &bad).is_err());
    }

    #[test]
    fn preset_newer_schema_is_rejected() {
        let dir = TempDir::new("newer");
        let path = dir.path("new.preset.negg.toml");
        fs::write(&path, format!("schema_version = 2\nfuture = 1\n{BODY}")).unwrap();
        let err = read_preset(&path).unwrap_err().to_string();
        assert!(err.contains("upgrade negg"), "{err}");
    }

    #[test]
    fn preset_with_bad_name_is_rejected() {
        let dir = TempDir::new("name");
        let mut bad = mine();
        bad.name = "a/b".into();
        assert!(write_preset(&dir.path("n.preset.negg.toml"), &bad).is_err());
    }
}
