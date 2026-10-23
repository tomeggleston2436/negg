// SPDX-License-Identifier: MPL-2.0

//! The error type for everything `negg-decode` reads.

use std::fmt;

/// Errors raised while decoding an input file.
#[derive(Debug)]
pub enum DecodeError {
    /// The file could not be opened or read.
    Io(std::io::Error),
    /// The `tiff` crate rejected the file; the string is its message.
    Tiff(String),
    /// rawler failed to decode the file; the string is its message.
    Raw(String),
    /// The file decoded but uses a layout this crate does not handle
    /// (greyscale, planar, X-Trans and so on). The string says which.
    Unsupported(String),
    /// The decoded buffer did not match the dimensions the file declared.
    Dimensions(negg_types::Error),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::Io(e) => write!(f, "I/O error: {e}"),
            DecodeError::Tiff(msg) => write!(f, "TIFF error: {msg}"),
            DecodeError::Raw(msg) => write!(f, "RAW error: {msg}"),
            DecodeError::Unsupported(what) => write!(f, "unsupported input: {what}"),
            DecodeError::Dimensions(e) => write!(f, "decoded buffer mismatch: {e}"),
        }
    }
}

impl std::error::Error for DecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DecodeError::Io(e) => Some(e),
            DecodeError::Dimensions(e) => Some(e),
            DecodeError::Tiff(_) | DecodeError::Raw(_) | DecodeError::Unsupported(_) => None,
        }
    }
}

impl From<std::io::Error> for DecodeError {
    fn from(e: std::io::Error) -> Self {
        DecodeError::Io(e)
    }
}

impl From<negg_types::Error> for DecodeError {
    fn from(e: negg_types::Error) -> Self {
        DecodeError::Dimensions(e)
    }
}

impl From<::tiff::TiffError> for DecodeError {
    fn from(e: ::tiff::TiffError) -> Self {
        match e {
            ::tiff::TiffError::IoError(io) => DecodeError::Io(io),
            other => DecodeError::Tiff(other.to_string()),
        }
    }
}

impl From<rawler::RawlerError> for DecodeError {
    fn from(e: rawler::RawlerError) -> Self {
        match e {
            rawler::RawlerError::Unsupported {
                what,
                model,
                make,
                mode,
            } => DecodeError::Unsupported(format!(
                "rawler: {what} (make '{make}', model '{model}', mode '{mode}')"
            )),
            rawler::RawlerError::DecoderFailed(msg) => DecodeError::Raw(msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_names_the_variant() {
        let io = DecodeError::from(std::io::Error::other("boom"));
        assert_eq!(io.to_string(), "I/O error: boom");
        assert_eq!(
            DecodeError::Tiff("bad".into()).to_string(),
            "TIFF error: bad"
        );
        assert_eq!(
            DecodeError::Raw("corrupt".into()).to_string(),
            "RAW error: corrupt"
        );
        assert_eq!(
            DecodeError::Unsupported("grey".into()).to_string(),
            "unsupported input: grey"
        );
        let dims = DecodeError::from(negg_types::Error::Dimensions {
            width: 1,
            height: 1,
            len: 2,
        });
        assert!(dims.to_string().starts_with("decoded buffer mismatch:"));
    }

    #[test]
    fn rawler_errors_map_by_kind() {
        let unsupported = DecodeError::from(rawler::RawlerError::Unsupported {
            what: "X-Trans".into(),
            model: "X-T1".into(),
            make: "Fujifilm".into(),
            mode: "".into(),
        });
        match unsupported {
            DecodeError::Unsupported(msg) => assert!(msg.contains("X-Trans"), "{msg}"),
            other => panic!("expected Unsupported, got {other:?}"),
        }
        let failed = DecodeError::from(rawler::RawlerError::DecoderFailed("bad".into()));
        match failed {
            DecodeError::Raw(msg) => assert_eq!(msg, "bad"),
            other => panic!("expected Raw, got {other:?}"),
        }
    }
}
