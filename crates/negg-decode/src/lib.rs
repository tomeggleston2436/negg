// SPDX-License-Identifier: MPL-2.0

//! RAW and TIFF decoding into linear camera RGB `f32` buffers.
//!
//! Owns the camera-side decode only. Must not invert, colour-manage
//! or write files. Phase 0 provides [`read_tiff`] for the reference
//! renders the evaluation compares against; RAW decoding via rawler
//! arrives in phase 1.

pub mod error;
pub mod tiff;

pub use error::DecodeError;
pub use tiff::{TiffImage, read_tiff};
