// SPDX-License-Identifier: Apache-2.0

//! Settings files (`--settings`, Ruling 33): a JSON file in the `Settings`
//! schema, or a sidecar whose `settings` are used.

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
    /// The file is a JSON object with none of the `Settings` keys and no
    /// `settings` key, so it is neither a settings file nor a sidecar.
    Unrecognised,
    /// The settings failed [`Settings::validate`].
    Invalid(negg_types::Error),
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SettingsError::Io(e) => write!(f, "cannot read: {e}"),
            SettingsError::Json(e) => write!(f, "{e}"),
            SettingsError::Sidecar(e) => write!(f, "read as a sidecar: {e}"),
            SettingsError::Unrecognised => write!(
                f,
                "not a settings file or a sidecar: no top-level {} or settings key",
                settings_keys().join(", ")
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
            SettingsError::Sidecar(e) => Some(e),
            SettingsError::Unrecognised => None,
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

/// Parses the text of a `--settings` file (Ruling 33): a JSON object with
/// a top-level `settings` key is a sidecar, loaded through
/// [`negg_io::migrate`], and its settings are used; an empty object, or an
/// object with at least one top-level `Settings` key, is a settings file
/// (omitted fields take their defaults, so `{}` is the defaults); anything
/// else is rejected. A settings file of schema version 1 is migrated to
/// the current version (Ruling 51). The result is validated.
///
/// # Errors
///
/// [`SettingsError::Json`] when the text is not JSON or does not fit the
/// `Settings` schema (including a value that is not an object),
/// [`SettingsError::Sidecar`] when a sidecar does not migrate,
/// [`SettingsError::Unrecognised`] for a non-empty object with no
/// recognised key,
/// and [`SettingsError::Invalid`] when the settings fail validation.
pub fn parse_settings(text: &str) -> Result<Settings, SettingsError> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(SettingsError::Json)?;
    let mut settings: Settings = match value.as_object() {
        Some(map) if map.contains_key("settings") => {
            negg_io::migrate(value)
                .map_err(SettingsError::Sidecar)?
                .settings
        }
        Some(map) if map.is_empty() || settings_keys().iter().any(|k| map.contains_key(k)) => {
            serde_json::from_value(value).map_err(SettingsError::Json)?
        }
        Some(_) => return Err(SettingsError::Unrecognised),
        None => serde_json::from_value(value).map_err(SettingsError::Json)?,
    };
    // A v1 settings file is migrated to the current schema (Ruling 51,
    // R-schema): v2 only adds fields, which take their defaults, so the
    // version number is the one thing to change. A newer version is kept
    // as it is.
    if settings.schema_version < SETTINGS_SCHEMA_VERSION {
        settings.schema_version = SETTINGS_SCHEMA_VERSION;
    }
    settings.validate().map_err(SettingsError::Invalid)?;
    Ok(settings)
}

/// Reads and parses the settings file at `path` ([`parse_settings`]).
///
/// # Errors
///
/// [`ConvertError::Settings`] naming `path` when the file cannot be read or
/// parsed.
pub fn read_settings_file(path: &Path) -> Result<Settings, ConvertError> {
    let wrap = |source| ConvertError::Settings {
        path: path.to_path_buf(),
        source,
    };
    let text = std::fs::read_to_string(path).map_err(|e| wrap(SettingsError::Io(e)))?;
    parse_settings(&text).map_err(wrap)
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
        assert_eq!(v2.schema_version, 3);
        assert_eq!(v1, v2);
        assert_eq!(v2, v3);
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
}
