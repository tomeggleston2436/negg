// SPDX-License-Identifier: MPL-2.0

//! Every per-pixel operation as a pure function on a buffer plus
//! settings: inversion, print curve, adjustments and scopes.
//!
//! Must not do I/O or decode, and must not hold a second copy of maths
//! that lives in another crate.
