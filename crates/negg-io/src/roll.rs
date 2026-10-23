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

use crate::error::{IoError, io_at};
use crate::sidecar::write_atomic;

/// File name of the roll sidecar inside a roll directory.
pub const ROLL_SIDECAR_NAME: &str = "roll.negg.json";

/// Schema version of the roll sidecar. It starts at 1, independent of the
/// frame sidecar's version (Ruling 51).
pub const ROLL_SIDECAR_SCHEMA_VERSION: u32 = 1;

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

/// Reads a roll sidecar.
///
/// # Errors
///
/// [`IoError::Io`] when the file cannot be read, [`IoError::Json`] when it
/// does not parse, and [`IoError::Schema`] when `schema_version` is not
/// [`ROLL_SIDECAR_SCHEMA_VERSION`].
pub fn read_roll_sidecar(path: &Path) -> Result<RollSidecar, IoError> {
    let text = fs::read_to_string(path).map_err(io_at(path))?;
    let sidecar: RollSidecar = serde_json::from_str(&text)?;
    if sidecar.schema_version != ROLL_SIDECAR_SCHEMA_VERSION {
        return Err(IoError::Schema(format!(
            "roll sidecar schema_version {} is not understood (this build reads version \
             {ROLL_SIDECAR_SCHEMA_VERSION})",
            sidecar.schema_version
        )));
    }
    Ok(sidecar)
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
    write_atomic(path, text.as_bytes())
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
        assert!(text.contains("\n  \"schema_version\": 1"));
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
        s.schema_version = 2;
        assert!(matches!(
            write_roll_sidecar(&path, &s),
            Err(IoError::Schema(_))
        ));
        fs::write(&path, r#"{"schema_version": 2}"#).unwrap();
        assert!(matches!(read_roll_sidecar(&path), Err(IoError::Schema(_))));
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
