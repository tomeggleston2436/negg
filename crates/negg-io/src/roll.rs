// SPDX-License-Identifier: MPL-2.0

//! The `roll.negg.json` sidecar for a roll directory (decision 4 of the
//! plan; roll pooling is step 3 and step 4 of docs/MATHS.md).
//!
//! It records the [`RollSettings`], their hash, the pooled estimates with
//! every frame's contribution and role, the frame file names, and the
//! negg version. Writing is atomic like [`crate::write_sidecar`].

use std::fs;
use std::path::{Path, PathBuf};

use negg_types::{ContentHash, RollPool, RollSettings};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{IoError, io_at};
use crate::sidecar::{schema_version_of, write_atomic};

/// File name of the roll sidecar inside a roll directory.
pub const ROLL_SIDECAR_NAME: &str = "roll.negg.json";

/// Schema version of the roll sidecar. It started at 1, independent of the
/// frame sidecar's version (Ruling 51); version 2 holds settings of schema
/// 3, with `geometry` (Ruling 71); version 3 holds settings of schema 4,
/// with `geometry.analysis_area`, and pool records that may carry
/// `needs_analysis_area` (Rulings 133 and 134); version 4 holds settings of
/// schema 5, with `print.saturation` and `print.preset` (ADR 0033).
pub const ROLL_SIDECAR_SCHEMA_VERSION: u32 = 4;

/// Everything negg records about one roll.
///
/// Every field has a default, so a file missing a field still loads.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct RollSidecar {
    /// Schema version of this file; [`ROLL_SIDECAR_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// The roll settings the frames were converted with.
    pub settings: RollSettings,
    /// [`RollSettings::content_hash`] of `settings` at the time of writing.
    pub settings_hash: ContentHash,
    /// The pooled estimates and per-frame roles, when pooling has run.
    pub pool: Option<RollPool>,
    /// File names of the roll's frames, with no directory component.
    pub frames: Vec<String>,
    /// Version of negg that wrote the file.
    pub negg_version: String,
}

impl Default for RollSidecar {
    fn default() -> Self {
        let settings = RollSettings::default();
        let settings_hash = settings.content_hash();
        RollSidecar {
            schema_version: ROLL_SIDECAR_SCHEMA_VERSION,
            settings,
            settings_hash,
            pool: None,
            frames: Vec::new(),
            negg_version: String::new(),
        }
    }
}

/// The roll sidecar path inside `dir`: `dir/roll.negg.json`.
pub fn roll_sidecar_path(dir: &Path) -> PathBuf {
    dir.join(ROLL_SIDECAR_NAME)
}

/// Reads a roll sidecar through [`migrate_roll`].
///
/// # Errors
///
/// [`IoError::Io`] when the file cannot be read, [`IoError::Json`] when it
/// does not parse, and whatever [`migrate_roll`] returns.
pub fn read_roll_sidecar(path: &Path) -> Result<RollSidecar, IoError> {
    let text = fs::read_to_string(path).map_err(io_at(path))?;
    let value: Value = serde_json::from_str(&text)?;
    migrate_roll(value)
}

/// Turns a parsed roll sidecar JSON value into the current [`RollSidecar`]
/// (Ruling 71), a ladder like [`crate::migrate`]: version 1 becomes 2 by
/// setting `schema_version = 2` and both `settings.schema_version` and
/// `settings.shared.schema_version` to 3, nothing else (the shared
/// `geometry` takes its default); version 2 becomes 3 by setting
/// `schema_version = 3` and both settings versions to 4, nothing else (no
/// analysis area, no frame flagged, Ruling 134); version 3 becomes 4 by
/// setting `schema_version = 4` and both settings versions to 5, nothing
/// else (saturation 1, no preset, ADR 0033). A missing
/// `schema_version` is read as the current version, so `{}` loads as the
/// default, as it always has.
///
/// # Errors
///
/// [`IoError::Schema`] when the value is not an object, `schema_version` is
/// not a non-negative integer, is 0, or is newer than
/// [`ROLL_SIDECAR_SCHEMA_VERSION`]; [`IoError::Json`] when the body does not
/// deserialise.
pub fn migrate_roll(value: Value) -> Result<RollSidecar, IoError> {
    let version = match value.as_object() {
        Some(object) if !object.contains_key("schema_version") => ROLL_SIDECAR_SCHEMA_VERSION,
        _ => schema_version_of(&value)?,
    };
    let value = match version {
        1 => upgrade_roll_v3_to_v4(upgrade_roll_v2_to_v3(upgrade_roll_v1_to_v2(value))),
        2 => upgrade_roll_v3_to_v4(upgrade_roll_v2_to_v3(value)),
        3 => upgrade_roll_v3_to_v4(value),
        ROLL_SIDECAR_SCHEMA_VERSION => value,
        v if v > ROLL_SIDECAR_SCHEMA_VERSION => {
            return Err(IoError::Schema(format!(
                "roll sidecar schema_version {v} is newer than this build understands (up to \
                 {ROLL_SIDECAR_SCHEMA_VERSION}); upgrade negg to read it"
            )));
        }
        other => {
            return Err(IoError::Schema(format!(
                "roll sidecar schema_version {other} is not a version negg has ever written"
            )));
        }
    };
    let mut sidecar: RollSidecar = serde_json::from_value(value)?;
    sidecar.schema_version = ROLL_SIDECAR_SCHEMA_VERSION;
    Ok(sidecar)
}

/// Upgrades a version 1 roll sidecar value to version 2 (Ruling 71).
fn upgrade_roll_v1_to_v2(value: Value) -> Value {
    set_roll_versions(value, 2, 3)
}

/// Upgrades a version 2 roll sidecar value to version 3 (Ruling 134).
fn upgrade_roll_v2_to_v3(value: Value) -> Value {
    set_roll_versions(value, 3, 4)
}

/// Upgrades a version 3 roll sidecar value to version 4 (ADR 0033): roll
/// version 4, settings version 5, nothing else (saturation 1, no preset).
fn upgrade_roll_v3_to_v4(value: Value) -> Value {
    set_roll_versions(value, 4, 5)
}

/// Sets a roll sidecar value's `schema_version` to `roll`, and both its
/// `settings.schema_version` (the roll settings) and its
/// `settings.shared.schema_version` to `settings`: the roll settings and
/// the shared settings follow the one settings schema.
fn set_roll_versions(mut value: Value, roll: u32, settings: u32) -> Value {
    if let Some(object) = value.as_object_mut() {
        object.insert("schema_version".into(), Value::from(roll));
        if let Some(roll_settings) = object.get_mut("settings").and_then(Value::as_object_mut) {
            roll_settings.insert("schema_version".into(), Value::from(settings));
            if let Some(shared) = roll_settings
                .get_mut("shared")
                .and_then(Value::as_object_mut)
            {
                shared.insert("schema_version".into(), Value::from(settings));
            }
        }
    }
    value
}

/// Writes a roll sidecar as pretty JSON with a trailing newline, atomically
/// (temporary file, sync, rename).
///
/// # Errors
///
/// [`IoError::Schema`] when `s.schema_version` is not
/// [`ROLL_SIDECAR_SCHEMA_VERSION`], [`IoError::InvalidSidecar`] when a frame
/// name contains a directory separator, [`IoError::Json`] and
/// [`IoError::Io`] otherwise.
pub fn write_roll_sidecar(path: &Path, s: &RollSidecar) -> Result<(), IoError> {
    write_atomic(path, &roll_sidecar_bytes(s)?).map(|_| ())
}

/// The bytes [`write_roll_sidecar`] writes for `s`: pretty JSON with a
/// trailing newline, for a caller that writes them itself with
/// [`crate::write_file_atomic`].
///
/// # Errors
///
/// As [`write_roll_sidecar`], without the file errors.
pub fn roll_sidecar_bytes(s: &RollSidecar) -> Result<Vec<u8>, IoError> {
    if s.schema_version != ROLL_SIDECAR_SCHEMA_VERSION {
        return Err(IoError::Schema(format!(
            "cannot write roll schema_version {}; this build writes version \
             {ROLL_SIDECAR_SCHEMA_VERSION}",
            s.schema_version
        )));
    }
    if let Some(bad) = s.frames.iter().find(|f| f.contains(['/', '\\'])) {
        return Err(IoError::InvalidSidecar(format!(
            "frame names must be file names without directories, got {bad:?}"
        )));
    }
    let mut text = serde_json::to_string_pretty(s)?;
    text.push('\n');
    Ok(text.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use negg_types::estimates::{CastCorrection, DminSource};
    use negg_types::{PoolFrame, PoolRole};

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "negg-io-roll-{name}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn sample() -> RollSidecar {
        let mut settings = RollSettings::default();
        settings.shared.print.exposure = 0.5;
        settings.pool_gamma = false;
        let settings_hash = settings.content_hash();
        let cast = CastCorrection {
            slope_r: 1.0,
            offset_r: 0.0,
            slope_b: 1.0,
            offset_b: 0.0,
            cells_used: 10,
        };
        RollSidecar {
            schema_version: ROLL_SIDECAR_SCHEMA_VERSION,
            settings,
            settings_hash,
            pool: Some(RollPool {
                dmin: Some([0.25, 0.5, 0.75]),
                gamma: None,
                cast: Some(cast),
                frames: vec![PoolFrame {
                    source: "a.dng".into(),
                    dmin: [0.25, 0.5, 0.75],
                    dmin_source: DminSource::Border,
                    dmax: [2.0, 2.0, 2.0],
                    gamma: [1.5, 1.5, 1.5],
                    cast,
                    dmin_role: PoolRole::Inlier,
                    gamma_role: PoolRole::Unused,
                    cast_role: PoolRole::Rejected,
                    needs_analysis_area: true,
                }],
            }),
            frames: vec!["a.dng".into(), "b.dng".into()],
            negg_version: "0.0.0-test".into(),
        }
    }

    #[test]
    fn path_is_inside_the_directory() {
        assert_eq!(
            roll_sidecar_path(Path::new("/scans/roll1")),
            PathBuf::from("/scans/roll1/roll.negg.json")
        );
    }

    #[test]
    fn round_trip_through_a_file() {
        let dir = TempDir::new("roundtrip");
        let path = roll_sidecar_path(&dir.0);
        write_roll_sidecar(&path, &sample()).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.ends_with('\n'));
        assert!(text.contains("\n  \"schema_version\": 4"));
        assert_eq!(read_roll_sidecar(&path).unwrap(), sample());
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1, "no temp file");
    }

    #[test]
    fn empty_object_loads_as_default() {
        let dir = TempDir::new("default");
        let path = roll_sidecar_path(&dir.0);
        fs::write(&path, "{}").unwrap();
        assert_eq!(read_roll_sidecar(&path).unwrap(), RollSidecar::default());
    }

    #[test]
    fn rejects_other_versions_and_bad_frame_names() {
        let dir = TempDir::new("reject");
        let path = roll_sidecar_path(&dir.0);
        let mut s = sample();
        s.schema_version = 5;
        assert!(matches!(
            write_roll_sidecar(&path, &s),
            Err(IoError::Schema(_))
        ));
        for bad in [r#"{"schema_version": 5}"#, r#"{"schema_version": 0}"#] {
            fs::write(&path, bad).unwrap();
            assert!(
                matches!(read_roll_sidecar(&path), Err(IoError::Schema(_))),
                "{bad}"
            );
        }
        let mut s = sample();
        s.frames.push("sub/c.dng".into());
        assert!(matches!(
            write_roll_sidecar(&path, &s),
            Err(IoError::InvalidSidecar(_))
        ));
    }

    #[test]
    fn read_reports_missing_file_and_bad_json() {
        let dir = TempDir::new("read");
        let path = roll_sidecar_path(&dir.0);
        assert!(matches!(read_roll_sidecar(&path), Err(IoError::Io { .. })));
        fs::write(&path, "{ nope").unwrap();
        assert!(matches!(read_roll_sidecar(&path), Err(IoError::Json(_))));
    }
}
