// SPDX-License-Identifier: MPL-2.0

//! Core types shared by every negg crate: the `f32` [`Image`] buffer,
//! per-frame [`Settings`], per-roll [`RollSettings`], the frame
//! [`Geometry`] (crop, analysis buffer, rotation, flip), the phase 2 rig and
//! film profile types, the [`ContentHash`]
//! that identifies a settings value, and the [`Error`] type.
//!
//! Must not depend on anything beyond serde, and must not contain
//! image-processing maths or I/O. [`Geometry::apply`] and [`Image::crop`]
//! only move pixels (Ruling 70).

pub mod error;
pub mod estimates;
pub mod geometry;
pub mod hash;
pub mod image;
pub mod profile;
pub mod rig;
pub mod roll;
pub mod settings;

pub use error::{Error, Result};
pub use estimates::{CastCorrection, CastSource, DminSource, Estimates, GammaSource};
pub use geometry::{ANALYSIS_BUFFER_DEFAULT, CropRect, Geometry};
pub use hash::ContentHash;
pub use image::Image;
pub use profile::{FilmProfile, RigProfile};
pub use rig::{DensityModel, FilmCorrection, FlatField, PatchRect, RigCorrection};
pub use roll::{FrameOverrides, PoolFrame, PoolRole, PooledEstimates, RollPool};
pub use settings::{CastEstimate, ProfileRefs, RollSettings, Settings};
