// SPDX-License-Identifier: MPL-2.0

//! Output writers for TIFF and JPEG, plus the `*.negg.json` sidecar.
//!
//! Owns file writing only. Must not decode input or contain rendering or
//! colour maths: the writers receive pixel data that negg-cms has already
//! encoded and quantised (ground rule 4 of the plan: quantisation happens
//! exactly once, in the exporter), and attach the metadata the plan asks
//! for (ICC profile, orientation, resolution).
//!
//! - [`write_tiff16`]: 16-bit RGB TIFF, Deflate by default, ICC in tag
//!   34675, Orientation 1, 300 pixels per inch.
//! - [`write_jpeg8`]: 8-bit RGB JPEG at quality 92 with the ICC profile in
//!   APP2.
//! - [`Sidecar`], [`sidecar_path`], [`read_sidecar`], [`write_sidecar`]
//!   and [`migrate`]: the schema-versioned sidecar beside each source.
//!
//! - [`RollSidecar`] and its read and write functions: the `roll.negg.json`
//!   beside a roll's frames.
//! - [`read_rig_profile`], [`write_rig_profile`], [`read_film_profile`] and
//!   [`write_film_profile`]: the `*.negg.toml` profiles (docs/PROFILES.md).
//! - [`parse_wedge_csv`] and [`parse_patches_csv`]: the calibration input
//!   files (docs/PROFILES.md, Input CSV files).
//!
//! Linear DNG output is phase 4 and is not here yet.

pub mod csv;
pub mod error;
pub mod jpeg;
pub mod profile;
pub mod roll;
pub mod sidecar;
pub mod tiff;

pub use csv::{
    NeutralPatchRef, WedgePatch, parse_patches_csv, parse_wedge_csv, read_patches_csv,
    read_wedge_csv,
};
pub use error::IoError;
pub use jpeg::{DEFAULT_JPEG_QUALITY, JpegOptions, write_jpeg8};
pub use profile::{read_film_profile, read_rig_profile, write_film_profile, write_rig_profile};
pub use roll::{
    ROLL_SIDECAR_NAME, ROLL_SIDECAR_SCHEMA_VERSION, RollSidecar, read_roll_sidecar,
    roll_sidecar_path, write_roll_sidecar,
};
pub use sidecar::{
    SIDECAR_SCHEMA_VERSION, SIDECAR_SUFFIX, Sidecar, migrate, read_sidecar, sidecar_path,
    write_sidecar,
};
pub use tiff::{Compression, DEFAULT_RESOLUTION_PPI, TiffOptions, write_tiff16};
