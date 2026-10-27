// SPDX-License-Identifier: MPL-2.0

//! The `*.negg.json` sidecar written beside each source frame (decision 4
//! of the plan).
//!
//! A sidecar records the [`Settings`] a frame was converted with, their
//! [`ContentHash`], the [`Estimates`] the analysis pass derived (Dmin, Dmax,
//! gammas and their sources, anchor, cast), the negg version that wrote
//! it, and a `schema_version` so that later builds can migrate old files.
//!
//! Reading always goes through [`migrate`], which inspects `schema_version`
//! before deserialising anything else. Writing is atomic: the JSON goes to
//! a temporary file of a unique name beside it and is renamed into place,
//! so a crash never leaves a half-written sidecar.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use negg_types::{ContentHash, Estimates, FrameOverrides, Settings};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{IoError, io_at};

/// The sidecar schema version this build writes and the newest it reads.
pub const SIDECAR_SCHEMA_VERSION: u32 = 5;

/// Extension chain appended to the source's stem to name its sidecar.
pub const SIDECAR_SUFFIX: &str = ".negg.json";

/// Everything negg records about one converted frame.
///
/// Every field has a default so that a sidecar missing a field still loads;
/// unknown fields are ignored so that a file written by a newer build with
/// the same `schema_version` still loads. A changed meaning of a field
/// needs a new `schema_version` and a step in [`migrate`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Sidecar {
    /// Schema version of this file. Always [`SIDECAR_SCHEMA_VERSION`] once
    /// loaded; [`migrate`] rejects anything else.
    pub schema_version: u32,
    /// File name of the source frame, with no directory component (the
    /// sidecar lives beside the source, so the directory is implied).
    pub source: String,
    /// The settings the frame was converted with.
    pub settings: Settings,
    /// [`Settings::content_hash`] of `settings` at the time of writing.
    pub settings_hash: ContentHash,
    /// The analysis estimates, when the frame has been analysed.
    pub estimates: Option<Estimates>,
    /// Per-frame overrides of the roll settings (`resolve` in negg-types
    /// merges them). Omitted from the file when empty.
    #[serde(skip_serializing_if = "FrameOverrides::is_empty")]
    pub overrides: FrameOverrides,
    /// File name of the roll sidecar (`roll.negg.json`) when the frame was
    /// rendered in roll mode, else `None` (omitted from the file).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roll: Option<String>,
    /// Version of negg that wrote the file (the caller passes its
    /// `CARGO_PKG_VERSION`; this crate does not read it itself).
    pub negg_version: String,
}

impl Default for Sidecar {
    fn default() -> Self {
        let settings = Settings::default();
        let settings_hash = settings.content_hash();
        Sidecar {
            schema_version: SIDECAR_SCHEMA_VERSION,
            source: String::new(),
            settings,
            settings_hash,
            estimates: None,
            overrides: FrameOverrides::default(),
            roll: None,
            negg_version: String::new(),
        }
    }
}

impl Sidecar {
    /// Builds a sidecar for `source` with `settings`, computing the hash.
    ///
    /// `source` is reduced to its file name; `estimates` may be `None` for a
    /// frame that has not been analysed yet.
    pub fn new(
        source: &Path,
        settings: Settings,
        estimates: Option<Estimates>,
        negg_version: &str,
    ) -> Sidecar {
        let settings_hash = settings.content_hash();
        Sidecar {
            schema_version: SIDECAR_SCHEMA_VERSION,
            source: source
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            settings,
            settings_hash,
            estimates,
            overrides: FrameOverrides::default(),
            roll: None,
            negg_version: negg_version.to_owned(),
        }
    }
}

/// The sidecar path for a source frame: the source's file stem (the name
/// with its last extension removed) plus `.negg.json`, in the same
/// directory.
///
/// `a/b/frame.CR2` gives `a/b/frame.negg.json`; `roll.01.frame.CR2` gives
/// `roll.01.frame.negg.json`; a name with no extension gets the suffix
/// appended.
pub fn sidecar_path(source: &Path) -> PathBuf {
    let stem = source
        .file_stem()
        .map(|s| s.to_os_string())
        .unwrap_or_default();
    let mut name = stem;
    name.push(SIDECAR_SUFFIX);
    source.with_file_name(name)
}

/// Reads and migrates a sidecar.
///
/// # Errors
///
/// [`IoError::Io`] when the file cannot be read, [`IoError::Json`] when it
/// is not JSON, and whatever [`migrate`] returns for the parsed value.
pub fn read_sidecar(path: &Path) -> Result<Sidecar, IoError> {
    let text = fs::read_to_string(path).map_err(io_at(path))?;
    let value: Value = serde_json::from_str(&text)?;
    migrate(value)
}

/// Writes a sidecar as pretty JSON with a trailing newline.
///
/// The JSON is written to a temporary file of a unique name in the same
/// directory (so the rename is within one file system), flushed to disk
/// with `sync_all`, and only then renamed over `path`, so a crash cannot
/// leave `path` naming a file whose contents never reached the disk. On any
/// failure the temporary file is removed and `path` is left as it was.
///
/// # Errors
///
/// [`IoError::Schema`] when `sidecar.schema_version` is not
/// [`SIDECAR_SCHEMA_VERSION`] (a struct with another version would write a
/// file this build could not read back), [`IoError::InvalidSidecar`] when
/// `source` contains a directory separator, [`IoError::Json`] when
/// serialisation fails and [`IoError::Io`] for file errors.
pub fn write_sidecar(path: &Path, sidecar: &Sidecar) -> Result<(), IoError> {
    write_atomic(path, &sidecar_bytes(sidecar)?).map(|_| ())
}

/// The bytes [`write_sidecar`] writes for `sidecar`: pretty JSON with a
/// trailing newline. A caller that writes them itself with
/// [`write_file_atomic`] (to know the stamp of what it wrote) gets the
/// very same file.
///
/// # Errors
///
/// As [`write_sidecar`], without the file errors.
pub fn sidecar_bytes(sidecar: &Sidecar) -> Result<Vec<u8>, IoError> {
    if sidecar.schema_version != SIDECAR_SCHEMA_VERSION {
        return Err(IoError::Schema(format!(
            "cannot write schema_version {}; this build writes version {SIDECAR_SCHEMA_VERSION}",
            sidecar.schema_version
        )));
    }
    if sidecar.source.contains(['/', '\\']) {
        return Err(IoError::InvalidSidecar(format!(
            "source must be a file name without directories, got {:?}",
            sidecar.source
        )));
    }
    let mut text = serde_json::to_string_pretty(sidecar)?;
    text.push('\n');
    Ok(text.into_bytes())
}

/// Writes `bytes` to `path` atomically: to a temporary file of a unique
/// name in the same directory, synced to disk, then renamed over `path`.
/// Returns the metadata of the file as written, taken from the open file
/// before the rename (a rename keeps the modification time and the
/// length), so a caller learns the stamp of its own write even when
/// another program replaces the file right after.
///
/// # Errors
///
/// [`IoError::Io`] for file errors; the temporary file is removed and
/// `path` is left as it was.
pub fn write_file_atomic(path: &Path, bytes: &[u8]) -> Result<fs::Metadata, IoError> {
    write_atomic(path, bytes)
}

/// Writes `bytes` atomically (see [`write_file_atomic`]). Shared by every
/// atomic write in this crate (sidecars, profiles and JPEGs).
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<fs::Metadata, IoError> {
    write_atomic_with(path, |file| file.write_all(bytes))
}

/// Creates a temporary file of a unique name beside `path`, lets `write`
/// fill it, syncs it to disk and renames it over `path`. The temporary
/// file is removed on any failure, and `path` is then left as it was; the
/// error names `path`. Returns the metadata of the synced file.
pub(crate) fn write_atomic_with(
    path: &Path,
    write: impl FnOnce(&mut fs::File) -> std::io::Result<()>,
) -> Result<fs::Metadata, IoError> {
    let tmp = temp_path(path);
    let result = (|| {
        let mut file = fs::File::options()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        write(&mut file)?;
        file.sync_all()?;
        file.metadata()
    })()
    .map_err(io_at(path))
    .and_then(|meta| fs::rename(&tmp, path).map(|()| meta).map_err(io_at(path)));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// A temporary name beside `path` that no other writer uses: the file
/// name, this process's id and a counter, so that two programs (the app
/// and the CLI) or two threads writing the same file never share one.
fn temp_path(path: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(format!(".{}.{n}.tmp", std::process::id()));
    path.with_file_name(name)
}

/// Turns a parsed sidecar JSON value into the current [`Sidecar`],
/// migrating older schema versions along the way.
///
/// The `schema_version` field is read first; only then is the rest of the
/// value interpreted. Migration is a ladder: a version `n` file is upgraded
/// to `n + 1` by a function that edits the raw `Value`, and so on until the
/// current version, which is deserialised. Version 3 adds the
/// settings' `geometry`, which takes its default; version 4
/// adds `geometry.analysis_area`, absent meaning none; version 5 (phase 4,
/// ADR 0033) adds `print.saturation` and `print.preset`, absent meaning 1
/// and none. Adding version 6 means bumping [`SIDECAR_SCHEMA_VERSION`],
/// writing `upgrade_v5_to_v6`, and adding it to the ladder below.
///
/// # Errors
///
/// [`IoError::Schema`] when `schema_version` is missing, is not a
/// non-negative integer, or is newer than this build understands;
/// [`IoError::Json`] when the body does not deserialise.
pub fn migrate(value: Value) -> Result<Sidecar, IoError> {
    let version = schema_version_of(&value)?;
    if version > SIDECAR_SCHEMA_VERSION {
        return Err(IoError::Schema(format!(
            "schema_version {version} is newer than this build understands \
             (up to {SIDECAR_SCHEMA_VERSION}); upgrade negg to read it"
        )));
    }
    let value = match version {
        1 => upgrade_v4_to_v5(upgrade_v3_to_v4(upgrade_v2_to_v3(upgrade_v1_to_v2(value)))),
        2 => upgrade_v4_to_v5(upgrade_v3_to_v4(upgrade_v2_to_v3(value))),
        3 => upgrade_v4_to_v5(upgrade_v3_to_v4(value)),
        4 => upgrade_v4_to_v5(value),
        5 => value,
        other => {
            return Err(IoError::Schema(format!(
                "schema_version {other} is not a sidecar version negg has ever written"
            )));
        }
    };
    let mut sidecar: Sidecar = serde_json::from_value(value)?;
    sidecar.schema_version = SIDECAR_SCHEMA_VERSION;
    Ok(sidecar)
}

/// Upgrades a version 1 sidecar value to version 2 (step 7 of
/// docs/MATHS.md for the cast source): sets both schema versions to 2 and,
/// when `estimates` is an object, inserts `neutral_offset = [0, 0, 0]` and
/// a `cast_source` of `fit` when `cast.cells_used > 0`, else `identity`.
fn upgrade_v1_to_v2(mut value: Value) -> Value {
    if let Some(object) = value.as_object_mut() {
        object.insert("schema_version".into(), Value::from(2));
        if let Some(settings) = object.get_mut("settings").and_then(Value::as_object_mut) {
            settings.insert("schema_version".into(), Value::from(2));
        }
        if let Some(estimates) = object.get_mut("estimates").and_then(Value::as_object_mut) {
            estimates.insert("neutral_offset".into(), serde_json::json!([0.0, 0.0, 0.0]));
            let fitted = estimates
                .get("cast")
                .and_then(|c| c.get("cells_used"))
                .and_then(Value::as_u64)
                .is_some_and(|n| n > 0);
            estimates.insert(
                "cast_source".into(),
                Value::from(if fitted { "fit" } else { "identity" }),
            );
        }
    }
    value
}

/// Upgrades a version 2 sidecar value to version 3: sets
/// `schema_version` and `settings.schema_version` to 3 and nothing else, so
/// the settings' `geometry` (and any frame override's) takes its default.
fn upgrade_v2_to_v3(mut value: Value) -> Value {
    if let Some(object) = value.as_object_mut() {
        object.insert("schema_version".into(), Value::from(3));
        if let Some(settings) = object.get_mut("settings").and_then(Value::as_object_mut) {
            settings.insert("schema_version".into(), Value::from(3));
        }
    }
    value
}

/// Upgrades a version 3 sidecar value to version 4: sets
/// `schema_version` and `settings.schema_version` to 4 and nothing else.
/// Version 3 has no `geometry.analysis_area`, and an absent one means none,
/// so every frame analyses exactly as it did.
fn upgrade_v3_to_v4(mut value: Value) -> Value {
    if let Some(object) = value.as_object_mut() {
        object.insert("schema_version".into(), Value::from(4));
        if let Some(settings) = object.get_mut("settings").and_then(Value::as_object_mut) {
            settings.insert("schema_version".into(), Value::from(4));
        }
    }
    value
}

/// Upgrades a version 4 sidecar value to version 5 (phase 4, ADR 0033):
/// sets `schema_version` and `settings.schema_version` to 5 and nothing
/// else. Version 4 has no `print.saturation` or `print.preset`, and absent
/// means 1 and none, so every frame renders exactly as it did.
fn upgrade_v4_to_v5(mut value: Value) -> Value {
    if let Some(object) = value.as_object_mut() {
        object.insert("schema_version".into(), Value::from(5));
        if let Some(settings) = object.get_mut("settings").and_then(Value::as_object_mut) {
            settings.insert("schema_version".into(), Value::from(5));
        }
    }
    value
}

pub(crate) fn schema_version_of(value: &Value) -> Result<u32, IoError> {
    let object = value
        .as_object()
        .ok_or_else(|| IoError::Schema("sidecar is not a JSON object".into()))?;
    let field = object
        .get("schema_version")
        .ok_or_else(|| IoError::Schema("schema_version is missing".into()))?;
    field
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(|| {
            IoError::Schema(format!(
                "schema_version must be a non-negative integer, got {field}"
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use negg_types::estimates::{CastCorrection, DminSource, GammaSource};
    use serde_json::json;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "negg-io-sidecar-{name}-{}-{}",
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

    fn sample_estimates() -> Estimates {
        Estimates {
            dmin: [0.21, 0.55, 0.78],
            dmin_source: DminSource::Border,
            dmax: [1.9, 2.2, 2.4],
            gamma: [1.69, 1.65, 1.62],
            gamma_source: GammaSource::Range,
            neutral_offset: [0.0; 3],
            anchor: -0.42,
            x0: 1.25,
            cast: CastCorrection {
                slope_r: 1.05,
                offset_r: -0.01,
                slope_b: 0.97,
                offset_b: 0.02,
                cells_used: 123,
            },
            cast_source: negg_types::CastSource::Fit,
        }
    }

    fn sample_sidecar() -> Sidecar {
        let mut settings = Settings::default();
        settings.print.exposure = 0.35;
        Sidecar::new(
            Path::new("/scans/roll.01.frame.CR2"),
            settings,
            Some(sample_estimates()),
            "0.0.0-test",
        )
    }

    #[test]
    fn sidecar_path_replaces_the_last_extension() {
        assert_eq!(
            sidecar_path(Path::new("a/b/frame.CR2")),
            PathBuf::from("a/b/frame.negg.json")
        );
        assert_eq!(
            sidecar_path(Path::new("frame.dng")),
            PathBuf::from("frame.negg.json")
        );
        assert_eq!(
            sidecar_path(Path::new("/scans/roll.01.frame.CR2")),
            PathBuf::from("/scans/roll.01.frame.negg.json")
        );
        assert_eq!(
            sidecar_path(Path::new("dir/noext")),
            PathBuf::from("dir/noext.negg.json")
        );
    }

    #[test]
    fn new_keeps_only_the_file_name_and_hashes_settings() {
        let sidecar = sample_sidecar();
        assert_eq!(sidecar.source, "roll.01.frame.CR2");
        assert_eq!(sidecar.schema_version, SIDECAR_SCHEMA_VERSION);
        assert_eq!(sidecar.settings_hash, sidecar.settings.content_hash());
        assert_ne!(
            sidecar.settings_hash,
            Settings::default().content_hash(),
            "the hash must reflect the edited settings"
        );
    }

    #[test]
    fn round_trip_through_a_file() {
        let dir = TempDir::new("roundtrip");
        let source = dir.path("roll.01.frame.CR2");
        let path = sidecar_path(&source);
        assert_eq!(path, dir.path("roll.01.frame.negg.json"));

        let sidecar = sample_sidecar();
        write_sidecar(&path, &sidecar).unwrap();

        let text = fs::read_to_string(&path).unwrap();
        assert!(text.ends_with('\n'), "trailing newline");
        assert!(text.contains("\n  \"schema_version\": 5"), "pretty printed");
        assert!(text.contains(&sidecar.settings_hash.to_hex()));
        assert!(!dir.path("roll.01.frame.negg.json.tmp").exists());

        let back = read_sidecar(&path).unwrap();
        assert_eq!(back, sidecar);
        assert_eq!(back.settings_hash, sidecar.settings_hash);
        assert_eq!(back.estimates, Some(sample_estimates()));
    }

    #[test]
    fn write_replaces_an_existing_sidecar_atomically() {
        let dir = TempDir::new("replace");
        let path = dir.path("frame.negg.json");
        let first = Sidecar::new(Path::new("frame.dng"), Settings::default(), None, "1");
        write_sidecar(&path, &first).unwrap();
        let second = sample_sidecar();
        write_sidecar(&path, &second).unwrap();
        assert_eq!(read_sidecar(&path).unwrap(), second);
        assert_eq!(
            fs::read_dir(&dir.0).unwrap().count(),
            1,
            "no temporary file left behind"
        );
    }

    #[test]
    fn write_failure_leaves_no_temp_file_and_no_sidecar() {
        let dir = TempDir::new("fail");
        let path = dir.path("missing/frame.negg.json");
        match write_sidecar(&path, &sample_sidecar()) {
            Err(IoError::Io { .. }) => {}
            other => panic!("expected Io error, got {other:?}"),
        }
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 0);
    }

    #[test]
    fn write_rejects_a_source_with_directories_or_another_version() {
        let dir = TempDir::new("reject");
        let path = dir.path("frame.negg.json");
        let mut bad = sample_sidecar();
        bad.source = "scans/frame.CR2".into();
        assert!(matches!(
            write_sidecar(&path, &bad),
            Err(IoError::InvalidSidecar(_))
        ));
        let mut future = sample_sidecar();
        future.schema_version = 6;
        assert!(matches!(
            write_sidecar(&path, &future),
            Err(IoError::Schema(_))
        ));
        assert!(!path.exists());
    }

    #[test]
    fn migrate_accepts_versions_1_to_5_and_fills_defaults() {
        for version in 1..=5 {
            let sidecar = migrate(json!({ "schema_version": version })).unwrap();
            assert_eq!(sidecar, Sidecar::default(), "{version}");
        }

        let partial = migrate(json!({
            "schema_version": 2,
            "source": "frame.dng",
            "settings": { "print": { "grade": 80.0 } },
            "future_field": true
        }))
        .unwrap();
        assert_eq!(partial.source, "frame.dng");
        assert_eq!(partial.settings.print.grade, 80.0);
        assert_eq!(partial.estimates, None);
    }

    /// A version 1 sidecar as phase 1 wrote it (no `neutral_offset`, no
    /// `cast_source`, no `overrides`, no `roll`).
    fn v1_fixture(cells_used: u32) -> Value {
        json!({
            "schema_version": 1,
            "source": "frame.dng",
            "settings": { "schema_version": 1, "print": { "exposure": 0.35 } },
            "settings_hash": Settings::default().content_hash().to_hex(),
            "estimates": {
                "dmin": [0.21, 0.55, 0.78],
                "dmin_source": "border",
                "dmax": [1.9, 2.2, 2.4],
                "gamma": [1.69, 1.65, 1.62],
                "gamma_source": "range",
                "anchor": -0.42,
                "x0": 1.25,
                "cast": {
                    "slope_r": 1.05, "offset_r": -0.01,
                    "slope_b": 0.97, "offset_b": 0.02,
                    "cells_used": cells_used
                }
            },
            "negg_version": "0.0.0-phase1"
        })
    }

    #[test]
    fn migrate_v1_adds_zero_offsets_and_fit_cast_source() {
        let s = migrate(v1_fixture(123)).unwrap();
        assert_eq!(s.schema_version, SIDECAR_SCHEMA_VERSION);
        assert_eq!(s.settings.schema_version, 5);
        assert!(s.settings.geometry.is_identity());
        assert_eq!(s.settings.print.exposure, 0.35);
        assert!(s.overrides.is_empty());
        assert_eq!(s.roll, None);
        let e = s.estimates.unwrap();
        assert_eq!(e.neutral_offset, [0.0; 3]);
        assert_eq!(e.cast_source, negg_types::CastSource::Fit);
        assert_eq!(e.cast.cells_used, 123);
        assert_eq!(e.dmin, [0.21, 0.55, 0.78]);
    }

    #[test]
    fn migrate_v1_with_no_cast_cells_gets_identity_cast_source() {
        let e = migrate(v1_fixture(0)).unwrap().estimates.unwrap();
        assert_eq!(e.cast_source, negg_types::CastSource::Identity);
        assert_eq!(e.neutral_offset, [0.0; 3]);
    }

    #[test]
    fn migrate_v1_without_estimates_or_settings_still_loads() {
        let s = migrate(json!({ "schema_version": 1, "source": "f.dng" })).unwrap();
        assert_eq!(s.estimates, None);
        assert_eq!(s.settings.schema_version, 5);
    }

    #[test]
    fn overrides_and_roll_round_trip_and_are_skipped_when_empty() {
        let dir = TempDir::new("overrides");
        let path = dir.path("frame.negg.json");

        let plain = sample_sidecar();
        write_sidecar(&path, &plain).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("\"overrides\""));
        assert!(!text.contains("\"roll\""));

        let mut rolled = sample_sidecar();
        rolled.overrides.print.grade = Some(75.0);
        rolled.roll = Some(crate::ROLL_SIDECAR_NAME.to_owned());
        write_sidecar(&path, &rolled).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"overrides\""));
        assert!(text.contains("\"roll\": \"roll.negg.json\""));
        assert_eq!(read_sidecar(&path).unwrap(), rolled);
    }

    #[test]
    fn migrate_rejects_unknown_and_missing_versions() {
        for (value, needle) in [
            (json!({ "schema_version": 6 }), "newer"),
            (json!({ "schema_version": 0 }), "ever written"),
            (json!({ "source": "frame.dng" }), "missing"),
            (json!({ "schema_version": "1" }), "non-negative integer"),
            (json!({ "schema_version": -1 }), "non-negative integer"),
            (json!({ "schema_version": 1.5 }), "non-negative integer"),
            (json!([1, 2]), "not a JSON object"),
        ] {
            match migrate(value.clone()) {
                Err(IoError::Schema(reason)) => {
                    assert!(reason.contains(needle), "{value}: {reason}");
                }
                other => panic!("{value}: expected Schema error, got {other:?}"),
            }
        }
    }

    #[test]
    fn migrate_reports_a_malformed_body_as_json() {
        let bad = json!({ "schema_version": 1, "settings_hash": "not-hex" });
        assert!(matches!(migrate(bad), Err(IoError::Json(_))));
    }

    #[test]
    fn read_reports_missing_file_and_bad_json() {
        let dir = TempDir::new("read");
        assert!(matches!(
            read_sidecar(&dir.path("absent.negg.json")),
            Err(IoError::Io { .. })
        ));
        let path = dir.path("garbage.negg.json");
        fs::write(&path, "{ not json").unwrap();
        assert!(matches!(read_sidecar(&path), Err(IoError::Json(_))));
    }

    #[test]
    fn serialised_field_names_are_the_contract_ones() {
        let value = serde_json::to_value(sample_sidecar()).unwrap();
        let object = value.as_object().unwrap();
        let mut keys: Vec<&String> = object.keys().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "estimates",
                "negg_version",
                "schema_version",
                "settings",
                "settings_hash",
                "source"
            ]
        );
        assert_eq!(object["estimates"]["dmin_source"], json!("border"));
    }

    #[test]
    fn concurrent_atomic_writes_use_their_own_temporary_files() {
        let dir = TempDir::new("concurrent");
        let path = dir.path("shared.negg.json");
        std::thread::scope(|scope| {
            for t in 0..8u8 {
                let path = &path;
                scope.spawn(move || {
                    for _ in 0..20 {
                        let meta = write_file_atomic(path, &[t; 64]).unwrap();
                        assert_eq!(meta.len(), 64);
                    }
                });
            }
        });
        let bytes = fs::read(&path).unwrap();
        assert_eq!(bytes.len(), 64);
        assert!(bytes.iter().all(|&b| b == bytes[0]), "one whole write");
        let names: Vec<String> = fs::read_dir(&dir.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["shared.negg.json"], "no temporary file is left");
        let bytes = sidecar_bytes(&sample_sidecar()).unwrap();
        write_sidecar(&path, &sample_sidecar()).unwrap();
        assert_eq!(fs::read(&path).unwrap(), bytes, "the same bytes");
    }
}
