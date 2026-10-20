// SPDX-License-Identifier: MPL-2.0

//! RAW and TIFF decoding into linear camera RGB `f32` buffers.
//!
//! Owns the camera-side decode only. Must not invert, colour-manage
//! or write files.
