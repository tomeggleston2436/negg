// SPDX-License-Identifier: MPL-2.0

//! The error type shared by everything in `negg-types`.
//!
//! The enum is deliberately small: other crates define their own errors and
//! wrap this one where they need to.

use std::fmt;

/// Errors raised by the types in this crate.
#[derive(Debug)]
pub enum Error {
    /// A pixel buffer's length does not match `width * height * CHANNELS`.
    Dimensions {
        /// Width the caller asked for, in pixels.
        width: usize,
        /// Height the caller asked for, in pixels.
        height: usize,
        /// Length of the buffer that was supplied, in floats.
        len: usize,
    },
    /// Settings failed validation; the string names the field and the rule.
    Invalid(String),
    /// A [`ContentHash`](crate::ContentHash) could not be parsed from text.
    Hex(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Dimensions { width, height, len } => write!(
                f,
                "pixel buffer of {len} floats does not match {width}x{height} pixels \
                 with {} channels",
                crate::image::CHANNELS
            ),
            Error::Invalid(reason) => write!(f, "invalid settings: {reason}"),
            Error::Hex(reason) => write!(f, "invalid content hash: {reason}"),
        }
    }
}

impl std::error::Error for Error {}

/// Result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_names_the_problem() {
        let e = Error::Dimensions {
            width: 2,
            height: 3,
            len: 5,
        };
        assert_eq!(
            e.to_string(),
            "pixel buffer of 5 floats does not match 2x3 pixels with 3 channels"
        );
        assert_eq!(
            Error::Invalid("grade".into()).to_string(),
            "invalid settings: grade"
        );
        assert_eq!(
            Error::Hex("short".into()).to_string(),
            "invalid content hash: short"
        );
    }
}
