// SPDX-License-Identifier: MPL-2.0

//! Colour management: the working space, ICC profile handling and
//! soft proof.
//!
//! Must not decode, invert or write files; it only transforms colour.
//! Phase 0 ships the pure colour science in [`lab`]: sRGB transfer
//! functions, XYZ, CIE 1976 L*a*b* and the CIEDE2000 colour difference,
//! which `negg eval` uses to score renders. ICC transforms and the
//! Rec.2020 working space arrive in phase 1.

pub mod lab;
