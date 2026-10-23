// SPDX-License-Identifier: MPL-2.0

//! The error type for every writer and sidecar function in this crate.

use std::fmt;
use std::path::PathBuf;

/// Errors raised by the writers and the sidecar functions.
#[derive(Debug)]
pub enum IoError {
    /// A file could not be created, written, read, renamed or removed.
    Io {
        /// The file the operation was working on.
        path: PathBuf,
        /// The underlying operating system error.
        source: std::io::Error,
    },
    /// The pixel buffer does not match the stated dimensions, or the
    /// dimensions themselves are unusable (zero, or larger than the format
    /// allows).
    Dimensions {
        /// Width the caller asked for, in pixels.
        width: usize,
        /// Height the caller asked for, in pixels.
        height: usize,
        /// Length of the buffer that was supplied, in samples.
        len: usize,
    },
    /// An option value is outside the range the format accepts (for example
    /// a JPEG quality of 0 or a resolution of 0 pixels per inch).
    InvalidOption(String),
    /// The `tiff` encoder reported an error.
    Tiff(tiff::TiffError),
    /// The `image` JPEG encoder reported an error.
    Jpeg(image::ImageError),
    /// A sidecar could not be parsed or produced as JSON.
    Json(serde_json::Error),
    /// A sidecar's `schema_version` is missing, is not a non-negative
    /// integer, or names a schema this build does not understand.
    Schema(String),
    /// A sidecar value is not writable as it stands (for example `source`
    /// contains a directory separator).
    InvalidSidecar(String),
    /// A rig or film profile could not be parsed as TOML, serialised, or
    /// failed `validate` (for example a `TODO(measure)` source).
    Profile(String),
    /// A wedge or patches CSV line is malformed.
    Csv {
        /// One-based line number in the file (comments and blank lines
        /// count).
        line: usize,
        /// What is wrong with the line.
        message: String,
    },
}

impl fmt::Display for IoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IoError::Io { path, source } => write!(f, "{}: {source}", path.display()),
            IoError::Dimensions { width, height, len } => write!(
                f,
                "pixel buffer of {len} samples does not match {width}x{height} pixels \
                 with 3 channels"
            ),
            IoError::InvalidOption(reason) => write!(f, "invalid option: {reason}"),
            IoError::Tiff(e) => write!(f, "TIFF encoding failed: {e}"),
            IoError::Jpeg(e) => write!(f, "JPEG encoding failed: {e}"),
            IoError::Json(e) => write!(f, "sidecar JSON error: {e}"),
            IoError::Schema(reason) => write!(f, "sidecar schema error: {reason}"),
            IoError::InvalidSidecar(reason) => write!(f, "invalid sidecar: {reason}"),
            IoError::Profile(reason) => write!(f, "profile error: {reason}"),
            IoError::Csv { line, message } => write!(f, "CSV line {line}: {message}"),
        }
    }
}

impl std::error::Error for IoError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            IoError::Io { source, .. } => Some(source),
            IoError::Tiff(e) => Some(e),
            IoError::Jpeg(e) => Some(e),
            IoError::Json(e) => Some(e),
            _ => None,
        }
    }
}

impl From<tiff::TiffError> for IoError {
    fn from(e: tiff::TiffError) -> Self {
        IoError::Tiff(e)
    }
}

impl From<image::ImageError> for IoError {
    fn from(e: image::ImageError) -> Self {
        IoError::Jpeg(e)
    }
}

impl From<serde_json::Error> for IoError {
    fn from(e: serde_json::Error) -> Self {
        IoError::Json(e)
    }
}

/// Wraps an I/O error with the path it concerns.
pub(crate) fn io_at(path: &std::path::Path) -> impl FnOnce(std::io::Error) -> IoError + '_ {
    move |source| IoError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Checks that `buffer_len` holds exactly `width * height * 3` samples and
/// that both dimensions are non-zero and fit the format's `u32` fields.
pub(crate) fn check_rgb_dimensions(
    width: usize,
    height: usize,
    buffer_len: usize,
) -> Result<(u32, u32), IoError> {
    let err = || IoError::Dimensions {
        width,
        height,
        len: buffer_len,
    };
    if width == 0 || height == 0 {
        return Err(err());
    }
    let expected = width
        .checked_mul(height)
        .and_then(|n| n.checked_mul(3))
        .ok_or_else(err)?;
    if expected != buffer_len {
        return Err(err());
    }
    let w = u32::try_from(width).map_err(|_| err())?;
    let h = u32::try_from(height).map_err(|_| err())?;
    Ok((w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dimensions_check_accepts_exact_and_rejects_everything_else() {
        assert_eq!(check_rgb_dimensions(2, 3, 18).unwrap(), (2, 3));
        assert!(matches!(
            check_rgb_dimensions(2, 3, 17),
            Err(IoError::Dimensions {
                width: 2,
                height: 3,
                len: 17
            })
        ));
        assert!(check_rgb_dimensions(0, 3, 0).is_err());
        assert!(check_rgb_dimensions(2, 0, 0).is_err());
        assert!(check_rgb_dimensions(usize::MAX, 2, 0).is_err());
    }

    #[test]
    fn display_names_the_problem() {
        let e = IoError::Dimensions {
            width: 2,
            height: 3,
            len: 5,
        };
        assert_eq!(
            e.to_string(),
            "pixel buffer of 5 samples does not match 2x3 pixels with 3 channels"
        );
        assert_eq!(
            IoError::Schema("v9".into()).to_string(),
            "sidecar schema error: v9"
        );
        let io = IoError::Io {
            path: PathBuf::from("/x/y.tif"),
            source: std::io::Error::other("boom"),
        };
        assert_eq!(io.to_string(), "/x/y.tif: boom");
        assert!(std::error::Error::source(&io).is_some());
    }
}
