// SPDX-License-Identifier: MPL-2.0

//! Rig profiles: flat-field correction, channel crosstalk and the
//! camera-to-density fit.
//!
//! Owns calibration maths and profile data. Must not decode files,
//! do I/O or render images.
