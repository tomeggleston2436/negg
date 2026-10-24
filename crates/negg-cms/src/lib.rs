// SPDX-License-Identifier: MPL-2.0

//! Colour management: the working space, ICC profile handling and
//! soft proof (step 7 of `docs/MATHS.md`).
//!
//! Must not decode, invert or write files; it only transforms colour.
//!
//! - [`lab`]: the pure colour science `negg eval` scores with: sRGB
//!   transfer functions, XYZ, CIE 1976 L*a*b* and CIEDE2000.
//! - [`matrix`]: 3x3 maths, the Bradford adaptation, the Rec.2020 matrices
//!   and [`camera_to_rec2020`], the white-normalised camera-to-Rec.2020
//!   matrix of ADR 0012. Since ADR 0026 RAW input does not use it for
//!   step 7 (it enters with [`REC709_TO_REC2020`]); it stays for the rig
//!   and print characterisation.
//! - [`profiles`]: the linear Rec.2020 working profile and the five output
//!   profiles, generated from primaries with Little-CMS.
//! - [`transform`]: Little-CMS transforms out of the working space to
//!   16-bit or 8-bit encoded RGB, the linearisation of a TIFF through its
//!   own matrix/TRC ICC profile (with the step 7 matrix out of its
//!   primaries), and the assumed gamma 2.2.
//!
//! Little-CMS objects are `Send` but not `Sync`; see [`transform`] for
//! how to use them from several threads. Soft proofing arrives with the
//! display path.

pub mod error;
pub mod lab;
pub mod matrix;
pub mod profiles;
pub mod transform;

pub use error::CmsError;
pub use matrix::{REC709_TO_REC2020, camera_to_rec2020};
pub use profiles::{output_profile, profile_bytes, working_profile};
pub use transform::{
    Intent, OutputTransform, gamma22_to_linear, icc_linearise_native, working_to_output,
};
