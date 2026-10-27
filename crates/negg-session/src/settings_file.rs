// SPDX-License-Identifier: Apache-2.0

//! Settings files (`--settings`): a JSON file in
//! the `Settings` schema, a frame sidecar whose `settings` are used, or a
//! roll sidecar (`roll.negg.json`) whose shared settings are used.

use std::fmt;
use std::path::Path;

use negg_io::IoError;
use negg_types::Settings;
use negg_types::settings::SETTINGS_SCHEMA_VERSION;

use crate::error::ConvertError;

/// Why a settings file could not be used.
#[derive(Debug)]
pub enum SettingsError {
    /// The file could not be read.
    Io(std::io::Error),
    /// The file is not JSON for the `Settings` schema.
    Json(serde_json::Error),
    /// The file has a top-level `settings` key, so it was read as a
    /// sidecar, and [`negg_io::migrate`] rejected it.
    Sidecar(IoError),
    /// The file has `settings.shared` (or a roll's `frames` or `pool`), so
    /// it was read as a roll sidecar, and [`negg_io::migrate_roll`]
    /// rejected it.
    RollSidecar(IoError),
    /// The file is a JSON object with none of the `Settings` keys and no
    /// `settings` key, or a sidecar whose `settings` have none of them, so
    /// it is neither a settings file nor a sidecar.
    Unrecognised,
    /// The settings' `schema_version` is newer than this build reads.
    Newer(u32),
    /// The settings failed [`Settings::validate`].
    Invalid(negg_types::Error),
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SettingsError::Io(e) => write!(f, "cannot read: {e}"),
            SettingsError::Json(e) => write!(f, "{e}"),
            SettingsError::Sidecar(e) => write!(f, "read as a sidecar: {e}"),
            SettingsError::RollSidecar(e) => write!(f, "read as a roll sidecar: {e}"),
            SettingsError::Unrecognised => write!(
                f,
                "not a settings file or a sidecar (of a frame or a roll): no top-level {} key, \
                 and no settings key with settings in it",
                settings_keys().join(", ")
            ),
            SettingsError::Newer(v) => write!(
                f,
                "settings schema_version {v} is newer than this build understands (up to \
                 {SETTINGS_SCHEMA_VERSION}); upgrade negg to read it"
            ),
            SettingsError::Invalid(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SettingsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SettingsError::Io(e) => Some(e),
            SettingsError::Json(e) => Some(e),
            SettingsError::Sidecar(e) | SettingsError::RollSidecar(e) => Some(e),
            SettingsError::Unrecognised | SettingsError::Newer(_) => None,
            SettingsError::Invalid(e) => Some(e),
        }
    }
}

/// The top-level keys of the `Settings` schema, read off the serialised
/// defaults so the list cannot drift from the struct.
fn settings_keys() -> Vec<String> {
    match serde_json::to_value(Settings::default()) {
        Ok(serde_json::Value::Object(map)) => map.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

/// A parsed `--settings` file: its settings and whether it gave a
/// geometry of its own (in roll mode the stored geometry is
/// kept unless it did).
#[derive(Clone, Debug, PartialEq)]
pub struct SettingsFile {
    /// The settings, migrated to the current schema and validated.
    pub settings: Settings,
    /// The settings object in the file has a `geometry` key.
    pub has_geometry: bool,
}

/// Parses the text of a `--settings` file: see
/// [`parse_settings_file`]; only the settings.
///
/// # Errors
///
/// As [`parse_settings_file`].
pub fn parse_settings(text: &str) -> Result<Settings, SettingsError> {
    parse_settings_file(text).map(|f| f.settings)
}

/// Parses the text of a `--settings` file:
///
/// - a JSON object with `settings.shared` (or a roll's top-level `frames`
///   or `pool`) is a roll sidecar, loaded through
///   [`negg_io::migrate_roll`], and its shared settings are used;
/// - a JSON object with a top-level `settings` key is a frame sidecar,
///   loaded through [`negg_io::migrate`], and its settings are used (a
///   `settings` object with none of the `Settings` keys is not
///   recognised);
/// - an empty object, or an object with at least one top-level `Settings`
///   key, is a settings file (omitted fields take their defaults, so `{}`
///   is the defaults);
/// - anything else is rejected.
///
/// A settings file of an older schema is migrated to the current version;
/// one of a newer schema than this build reads is rejected.
/// The result is validated.
///
/// # Errors
///
/// [`SettingsError::Json`] when the text is not JSON or does not fit the
/// `Settings` schema (including a value that is not an object),
/// [`SettingsError::Sidecar`] or [`SettingsError::RollSidecar`] when a
/// sidecar does not migrate, [`SettingsError::Unrecognised`] for an
/// object with no recognised key, [`SettingsError::Newer`] for a newer
/// schema, and [`SettingsError::Invalid`] when the settings fail
/// validation.
pub fn parse_settings_file(text: &str) -> Result<SettingsFile, SettingsError> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(SettingsError::Json)?;
    let recognised = |m: &serde_json::Map<String, serde_json::Value>| {
        m.is_empty() || settings_keys().iter().any(|k| m.contains_key(k))
    };
    let has_geometry = |v: Option<&serde_json::Value>| v.and_then(|v| v.get("geometry")).is_some();
    let (mut settings, geometry): (Settings, bool) = match value.as_object() {
        Some(map)
            if map.get("settings").and_then(|s| s.get("shared")).is_some()
                || map.contains_key("frames")
                || map.contains_key("pool") =>
        {
            let shared = map.get("settings").and_then(|s| s.get("shared"));
            let geometry = has_geometry(shared);
            if let Some(v) = newer(shared) {
                return Err(SettingsError::Newer(v));
            }
            let roll = negg_io::migrate_roll(value).map_err(SettingsError::RollSidecar)?;
            (roll.settings.shared, geometry)
        }
        Some(map) if map.contains_key("settings") => {
            let inner = map.get("settings");
            if let Some(m) = inner.and_then(serde_json::Value::as_object)
                && !recognised(m)
            {
                return Err(SettingsError::Unrecognised);
            }
            let geometry = has_geometry(inner);
            if let Some(v) = newer(inner) {
                return Err(SettingsError::Newer(v));
            }
            let sidecar = negg_io::migrate(value).map_err(SettingsError::Sidecar)?;
            (sidecar.settings, geometry)
        }
        Some(map) if recognised(map) => {
            let geometry = map.contains_key("geometry");
            (
                serde_json::from_value(value).map_err(SettingsError::Json)?,
                geometry,
            )
        }
        Some(_) => return Err(SettingsError::Unrecognised),
        None => (
            serde_json::from_value(value).map_err(SettingsError::Json)?,
            false,
        ),
    };
    if settings.schema_version > SETTINGS_SCHEMA_VERSION {
        return Err(SettingsError::Newer(settings.schema_version));
    }
    // An older settings file is migrated to the current schema:
    // v2, v3 and v4 only add fields, which take their defaults, so the
    // version number is the one thing to change.
    settings.schema_version = SETTINGS_SCHEMA_VERSION;
    settings.validate().map_err(SettingsError::Invalid)?;
    Ok(SettingsFile {
        settings,
        has_geometry: geometry,
    })
}

/// The `schema_version` of a settings object when it is newer than this
/// build reads.
fn newer(settings: Option<&serde_json::Value>) -> Option<u32> {
    settings
        .and_then(|s| s.get("schema_version"))
        .and_then(serde_json::Value::as_u64)
        .filter(|&v| v > u64::from(SETTINGS_SCHEMA_VERSION))
        .map(|v| u32::try_from(v).unwrap_or(u32::MAX))
}

/// Reads and parses the settings file at `path` ([`parse_settings`]).
///
/// # Errors
///
/// [`ConvertError::Settings`] naming `path` when the file cannot be read or
/// parsed.
pub fn read_settings_file(path: &Path) -> Result<Settings, ConvertError> {
    read_settings_file_parts(path).map(|f| f.settings)
}

/// Reads and parses the settings file at `path` ([`parse_settings_file`]).
///
/// # Errors
///
/// As [`read_settings_file`].
pub fn read_settings_file_parts(path: &Path) -> Result<SettingsFile, ConvertError> {
    let wrap = |source| ConvertError::Settings {
        path: path.to_path_buf(),
        source,
    };
    let text = std::fs::read_to_string(path).map_err(|e| wrap(SettingsError::Io(e)))?;
    parse_settings_file(&text).map_err(wrap)
}

#[cfg(test)]
mod tests {
    use super::*;
    use negg_io::Sidecar;

    #[test]
    fn settings_text_is_a_settings_file_or_a_sidecar() {
        let s = parse_settings(r#"{"print": {"grade": 90}}"#).unwrap();
        assert_eq!(s.print.grade, 90.0);
        let mut sidecar_settings = Settings::default();
        sidecar_settings.print.grade = 60.0;
        let sidecar = Sidecar::new(Path::new("a.dng"), sidecar_settings.clone(), None, "0.0.0");
        let text = serde_json::to_string(&sidecar).unwrap();
        assert_eq!(parse_settings(&text).unwrap(), sidecar_settings);
        // A sidecar that does not migrate is reported as one.
        let err = parse_settings(r#"{"settings": {}, "schema_version": 99}"#).unwrap_err();
        assert!(matches!(err, SettingsError::Sidecar(_)), "{err:?}");
        assert!(err.to_string().starts_with("read as a sidecar:"), "{err}");
        // The empty object is a settings file with every field defaulted.
        assert_eq!(parse_settings("{}").unwrap(), Settings::default());
        // Neither: rejected.
        let err = parse_settings(r#"{"grade": 60}"#).unwrap_err();
        assert!(matches!(err, SettingsError::Unrecognised), "{err:?}");
        assert!(err.to_string().contains("colour"), "{err}");
        assert!(matches!(
            parse_settings("[1, 2]"),
            Err(SettingsError::Json(_))
        ));
        assert!(matches!(
            parse_settings(r#"{"print": {"grade": 500}}"#),
            Err(SettingsError::Invalid(_))
        ));
    }

    #[test]
    fn a_v1_settings_file_is_migrated_to_the_current_schema() {
        let v1 = parse_settings(r#"{"schema_version": 1, "print": {"grade": 70}}"#).unwrap();
        let v2 = parse_settings(r#"{"schema_version": 2, "print": {"grade": 70}}"#).unwrap();
        let v3 = parse_settings(r#"{"schema_version": 3, "print": {"grade": 70}}"#).unwrap();
        assert_eq!(v1.schema_version, SETTINGS_SCHEMA_VERSION);
        let v4 = parse_settings(r#"{"schema_version": 4, "print": {"grade": 70}}"#).unwrap();
        assert_eq!(v2.schema_version, SETTINGS_SCHEMA_VERSION);
        assert_eq!(v1, v2);
        assert_eq!(v2, v3);
        assert_eq!(v3, v4);
        assert_eq!(v3.geometry.analysis_area, None);
        assert_eq!(v1.content_hash(), v2.content_hash());
        assert!(v2.geometry.is_identity());
    }

    #[test]
    fn a_bad_geometry_in_a_settings_file_is_invalid() {
        assert!(matches!(
            parse_settings(r#"{"geometry": {"rotate_degrees": 45}}"#),
            Err(SettingsError::Invalid(_))
        ));
        let s = parse_settings(
            r#"{"geometry": {"crop": {"x": 0.1, "y": 0.1, "width": 0.5, "height": 0.5}}}"#,
        )
        .unwrap();
        assert!(s.geometry.crop.is_some());
        let s = parse_settings(
            r#"{"geometry": {"analysis_area": {"x": 0.1, "y": 0.1, "width": 0.5, "height": 0.5}}}"#,
        )
        .unwrap();
        assert!(s.geometry.analysis_area.is_some() && s.geometry.crop.is_none());
        assert!(matches!(
            parse_settings(
                r#"{"geometry": {"analysis_area": {"x": 0.6, "y": 0.1, "width": 0.5, "height": 0.5}}}"#,
            ),
            Err(SettingsError::Invalid(_))
        ));
    }

    #[test]
    fn a_missing_settings_file_names_its_path() {
        let err = read_settings_file(Path::new("/nonexistent/s.json")).unwrap_err();
        assert!(err.is_usage_error());
        assert!(
            err.to_string()
                .starts_with("settings /nonexistent/s.json: cannot read:"),
            "{err}"
        );
    }

    #[test]
    fn a_roll_sidecar_gives_its_shared_settings() {
        let mut roll = negg_types::RollSettings::default();
        roll.shared.print.grade = 75.0;
        roll.shared.geometry.analysis_area = Some(negg_types::CropRect {
            x: 0.1,
            y: 0.1,
            width: 0.8,
            height: 0.8,
        });
        roll.pool_gamma = false;
        let sidecar = negg_io::RollSidecar {
            settings: roll.clone(),
            frames: vec!["a.dng".into()],
            ..negg_io::RollSidecar::default()
        };
        let text = serde_json::to_string_pretty(&sidecar).unwrap();
        let file = parse_settings_file(&text).unwrap();
        assert_eq!(file.settings, roll.shared);
        assert!(file.has_geometry);
        // A frame sidecar whose settings hold a roll's keys is not taken
        // for defaults.
        let err = parse_settings(r#"{"settings": {"pool_gamma": false}}"#).unwrap_err();
        assert!(matches!(err, SettingsError::Unrecognised), "{err:?}");
        // A plain settings file without geometry keeps the stored one.
        let file = parse_settings_file(r#"{"print": {"grade": 80}}"#).unwrap();
        assert!(!file.has_geometry);
        let file = parse_settings_file(r#"{"geometry": {"rotate_degrees": 90}}"#).unwrap();
        assert!(file.has_geometry);
        // A roll sidecar of a newer schema is a roll sidecar error.
        let err = parse_settings(r#"{"schema_version": 9, "frames": []}"#).unwrap_err();
        assert!(matches!(err, SettingsError::RollSidecar(_)), "{err:?}");
    }

    #[test]
    fn newer_settings_are_rejected() {
        let v = SETTINGS_SCHEMA_VERSION + 1;
        for text in [
            format!(r#"{{"schema_version": {v}, "print": {{"grade": 70}}}}"#),
            format!(r#"{{"schema_version": 4, "settings": {{"schema_version": {v}}}}}"#),
            format!(
                r#"{{"settings": {{"schema_version": 4, "shared": {{"schema_version": {v}}}}}}}"#
            ),
        ] {
            let err = parse_settings(&text).unwrap_err();
            assert!(
                matches!(err, SettingsError::Newer(n) if n == v),
                "{text}: {err:?}"
            );
            assert!(err.to_string().contains("upgrade negg"), "{err}");
        }
    }
}
