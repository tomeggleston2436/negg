// SPDX-License-Identifier: MPL-2.0

//! Core types shared by every negg crate: `Image<f32>`, `Settings`,
//! `RollSettings` and the error types.
//!
//! Must not depend on anything beyond serde, and must not contain
//! image-processing maths or I/O.
