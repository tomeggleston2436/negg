// SPDX-License-Identifier: Apache-2.0

//! [`ConvertError`]: why a conversion, a batch, a profile load or a lock
//! failed.

use std::fmt;
use std::path::PathBuf;

use negg_cms::CmsError;
use negg_decode::DecodeError;
use negg_io::IoError;

use crate::settings_file::SettingsError;

/// Why a conversion failed.
#[derive(Debug)]
pub enum ConvertError {
    /// The `--settings` file could not be used (a usage error).
    Settings {
        /// The settings file.
        path: PathBuf,
        /// What went wrong.
        source: SettingsError,
    },
    /// `negg batch` was given something that is not a directory (a usage
    /// error).
    NotADirectory(PathBuf),
    /// The input could not be decoded.
    Decode(DecodeError),
    /// Linearisation, the camera matrix or the export transform failed.
    Cms(CmsError),
    /// The render failed (invalid settings for this frame, or an empty
    /// image).
    Render(negg_types::Error),
    /// The output image or the sidecar could not be written.
    Write(IoError),
    /// A directory could not be created or read.
    Io {
        /// The path concerned.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
    /// The output image or the sidecar would be written over the input.
    /// Refused before anything is decoded or written.
    WouldOverwriteSource {
        /// The input, which is left untouched.
        input: PathBuf,
        /// The output or sidecar path that resolves to the input.
        target: PathBuf,
    },
    /// In `negg batch`, an earlier source in the same run already wrote
    /// this output path (for example `a.tif` and `a.tiff`), so this source
    /// is skipped rather than overwrite it.
    DuplicateOutput {
        /// The output path both sources resolve to.
        output: PathBuf,
        /// The source that produced it first.
        first: PathBuf,
    },
    /// The frame's sidecar path would be the roll sidecar
    /// (`roll.negg.json`, R-roll), so the frame is refused.
    RollSidecarClash(PathBuf),
    /// A rig or film profile, or a calibration CSV, could not be read (a
    /// usage error).
    Profile {
        /// The file concerned.
        path: PathBuf,
        /// What went wrong.
        source: IoError,
    },
    /// The roll sidecar could not be read or written.
    RollSidecar {
        /// The roll sidecar.
        path: PathBuf,
        /// What went wrong.
        source: IoError,
    },
    /// The frame's existing sidecar could not be read or parsed. The frame
    /// is not rendered, so the sidecar (and any locks or overrides in it)
    /// and the output are left untouched (R-lock).
    FrameSidecar {
        /// The frame's sidecar.
        path: PathBuf,
        /// What went wrong.
        source: IoError,
    },
    /// A rig measurement failed (negg-calib).
    Calib(negg_calib::CalibError),
    /// The command line is inconsistent (a usage error), for example band
    /// frames given without all three.
    Usage(String),
    /// A measurement or lock could not be made from the data, for example a
    /// patch with too few grid cells or a frame with no estimates.
    Invalid(String),
}

impl ConvertError {
    /// Whether the error is in the command line or the settings file
    /// rather than in the conversion; the binary maps these to the usage
    /// exit status.
    pub fn is_usage_error(&self) -> bool {
        matches!(
            self,
            ConvertError::Settings { .. }
                | ConvertError::NotADirectory(_)
                | ConvertError::Profile { .. }
                | ConvertError::Usage(_)
        )
    }
}

impl fmt::Display for ConvertError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConvertError::Settings { path, source } => {
                write!(f, "settings {}: {source}", path.display())
            }
            ConvertError::NotADirectory(path) => {
                write!(f, "{} is not a directory", path.display())
            }
            ConvertError::Decode(e) => write!(f, "decode: {e}"),
            ConvertError::Cms(e) => write!(f, "colour management: {e}"),
            ConvertError::Render(e) => write!(f, "render: {e}"),
            ConvertError::Write(e) => write!(f, "write: {e}"),
            ConvertError::Io { path, source } => write!(f, "{}: {source}", path.display()),
            ConvertError::WouldOverwriteSource { input, target } => write!(
                f,
                "refusing to write {} over the source {}",
                target.display(),
                input.display()
            ),
            ConvertError::DuplicateOutput { output, first } => write!(
                f,
                "{}: output path already produced by {} in this run",
                output.display(),
                first.display()
            ),
            ConvertError::RollSidecarClash(input) => write!(
                f,
                "refusing {}: its sidecar would be the roll sidecar {}",
                input.display(),
                negg_io::ROLL_SIDECAR_NAME
            ),
            ConvertError::Profile { path, source } => {
                write!(f, "profile {}: {source}", path.display())
            }
            ConvertError::RollSidecar { path, source } => {
                write!(f, "roll sidecar {}: {source}", path.display())
            }
            ConvertError::FrameSidecar { path, source } => write!(
                f,
                "cannot read the existing sidecar {} ({source}); the frame is not converted \
                 and the sidecar is left as it is: fix or remove it",
                path.display()
            ),
            ConvertError::Calib(e) => write!(f, "calibration: {e}"),
            ConvertError::Usage(m) | ConvertError::Invalid(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for ConvertError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConvertError::Settings { source, .. } => Some(source),
            ConvertError::NotADirectory(_) => None,
            ConvertError::Decode(e) => Some(e),
            ConvertError::Cms(e) => Some(e),
            ConvertError::Render(e) => Some(e),
            ConvertError::Write(e) => Some(e),
            ConvertError::Io { source, .. } => Some(source),
            ConvertError::Profile { source, .. }
            | ConvertError::RollSidecar { source, .. }
            | ConvertError::FrameSidecar { source, .. } => Some(source),
            ConvertError::Calib(e) => Some(e),
            ConvertError::WouldOverwriteSource { .. }
            | ConvertError::DuplicateOutput { .. }
            | ConvertError::RollSidecarClash(_)
            | ConvertError::Usage(_)
            | ConvertError::Invalid(_) => None,
        }
    }
}

impl From<DecodeError> for ConvertError {
    fn from(e: DecodeError) -> Self {
        ConvertError::Decode(e)
    }
}

impl From<CmsError> for ConvertError {
    fn from(e: CmsError) -> Self {
        ConvertError::Cms(e)
    }
}

impl From<negg_types::Error> for ConvertError {
    fn from(e: negg_types::Error) -> Self {
        ConvertError::Render(e)
    }
}

impl From<negg_calib::CalibError> for ConvertError {
    fn from(e: negg_calib::CalibError) -> Self {
        ConvertError::Calib(e)
    }
}

impl From<IoError> for ConvertError {
    fn from(e: IoError) -> Self {
        ConvertError::Write(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_errors_are_classified() {
        let e = ConvertError::NotADirectory(PathBuf::from("x"));
        assert!(e.is_usage_error());
        assert_eq!(e.to_string(), "x is not a directory");
        let e = ConvertError::Settings {
            path: PathBuf::from("s.json"),
            source: SettingsError::Invalid(negg_types::Error::Invalid("grade".into())),
        };
        assert!(e.is_usage_error());
        assert_eq!(e.to_string(), "settings s.json: invalid settings: grade");
        let e = ConvertError::Render(negg_types::Error::Invalid("x0".into()));
        assert!(!e.is_usage_error());
        assert!(e.to_string().starts_with("render:"));
    }
}
