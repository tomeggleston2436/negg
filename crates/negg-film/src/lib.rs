// SPDX-License-Identifier: MPL-2.0

//! Film profiles, Dmin/Dmax/gamma estimators and roll pooling.
//!
//! Owns per-film and per-roll statistics. Must not do I/O, decode or
//! per-pixel rendering.
