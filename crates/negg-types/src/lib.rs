// SPDX-License-Identifier: MPL-2.0

//! Core types shared by every negg crate: the `f32` [`Image`] buffer,
//! per-frame [`Settings`], per-roll [`RollSettings`], the [`ContentHash`]
//! that identifies a settings value, and the [`Error`] type.
//!
//! Must not depend on anything beyond serde, and must not contain
//! image-processing maths or I/O.

pub mod error;
pub mod hash;
pub mod image;
pub mod settings;

pub use error::{Error, Result};
pub use hash::ContentHash;
pub use image::Image;
pub use settings::{RollSettings, Settings};
