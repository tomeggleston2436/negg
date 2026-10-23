// SPDX-License-Identifier: MPL-2.0

//! RAW and TIFF decoding into linear camera RGB `f32` buffers.
//!
//! Owns the camera-side decode only. Must not invert, colour-manage
//! or write files. [`decode`] dispatches on the file extension: `.tif`
//! and `.tiff` go through the `tiff` crate ([`decode_tiff`]), everything
//! else through rawler ([`decode_raw`]). [`read_tiff`] remains available
//! for the reference renders the evaluation compares against.
//!
//! A [`Decoded`] image is linear only when its [`Transfer`] is
//! [`Transfer::Linear`]; a TIFF without a usable transfer description is
//! returned with its encoded values and a transfer the caller must undo.

pub mod demosaic;
pub mod error;
pub mod levels;
pub mod raw;
pub mod tiff;

#[cfg(test)]
pub(crate) mod testutil;

use std::path::Path;

use negg_types::Image;

pub use error::DecodeError;
pub use raw::{D65_WHITE, decode_raw, illuminant_white};
pub use tiff::{TiffImage, decode_tiff, read_tiff};

/// How the values in a [`Decoded`] image are encoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Transfer {
    /// The values are linear light. Always the case for RAW input and for
    /// floating-point TIFFs.
    Linear,
    /// An integer TIFF with no ICC profile: the values are assumed to be
    /// gamma 2.2 encoded and are returned as read. A warning says so.
    Gamma22Assumed,
    /// The TIFF carries an embedded ICC profile (these bytes). The values
    /// are returned as read; the caller linearises through `negg-cms`.
    Icc(Vec<u8>),
}

/// A decoded input image plus the metadata downstream steps need.
#[derive(Clone, Debug, PartialEq)]
pub struct Decoded {
    /// For RAW input: linear camera RGB after black and white scaling,
    /// clipped below at 0 and allowed above 1 where the sensor exceeds its
    /// calibrated white level. For TIFF input: the file's values in
    /// `[0, 1]` (or as stored, for float), encoded per `transfer`.
    pub image: Image,
    /// The encoding of `image`; `Linear` for RAW.
    pub transfer: Transfer,
    /// rawler's XYZ-to-camera matrix, for the D65 illuminant when the file
    /// has one (rows are camera channels). Never applied here; step 7
    /// turns it into the camera-to-Rec.2020 matrix.
    pub xyz_to_camera: Option<[[f32; 3]; 3]>,
    /// CIE XYZ (`Y = 1`) of the illuminant `xyz_to_camera` was measured
    /// under: D65 `[0.95047, 1, 1.08883]`, D50, D55, D75 or A from the CIE
    /// tables, or D65 with a warning for any other illuminant. `Some`
    /// exactly when `xyz_to_camera` is (ADR 0012).
    pub matrix_white: Option<[f32; 3]>,
    /// The camera's make and model as rawler reports them.
    pub camera: Option<String>,
    /// The per-channel calibrated white level, in the file's sample units,
    /// that was mapped to 1.0. `[1, 1, 1]` for TIFF input.
    pub white_level: [f32; 3],
    /// The per-channel calibrated black level that was mapped to 0.0.
    /// `[0, 0, 0]` for TIFF input.
    pub black_level: [f32; 3],
    /// Human-readable notes about assumptions made while decoding.
    pub warnings: Vec<String>,
}

/// Decodes a file, choosing the reader by extension.
///
/// `.tif` and `.tiff` (any letter case) are read as TIFF; everything else
/// is handed to rawler. A TIFF that rawler would also understand (a DNG
/// named `.tif`) is still read as a plain TIFF, and a failure on either
/// path is returned as is with no retry on the other.
///
/// # Errors
///
/// Whatever [`decode_tiff`] or [`decode_raw`] returns.
pub fn decode(path: &Path) -> Result<Decoded, DecodeError> {
    if is_tiff_extension(path) {
        decode_tiff(path)
    } else {
        decode_raw(path)
    }
}

/// Whether the path's extension is `tif` or `tiff`, ignoring case.
fn is_tiff_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("tif") || e.eq_ignore_ascii_case("tiff"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{DngSpec, TempDir, mosaic_from_scene};
    use ::tiff::encoder::TiffEncoder;
    use ::tiff::encoder::colortype::RGB16;
    use std::fs::File;
    use std::path::PathBuf;

    #[test]
    fn extension_dispatch_is_case_insensitive() {
        for name in ["a.tif", "a.TIF", "a.tiff", "a.TiFf"] {
            assert!(is_tiff_extension(&PathBuf::from(name)), "{name}");
        }
        for name in ["a.dng", "a.CR2", "a.tif.bak", "tif", "a"] {
            assert!(!is_tiff_extension(&PathBuf::from(name)), "{name}");
        }
    }

    #[test]
    fn decode_routes_tiff_and_raw() {
        let dir = TempDir::new("dispatch");

        let tif = dir.path("plain.TIF");
        let mut enc = TiffEncoder::new(File::create(&tif).unwrap()).unwrap();
        enc.write_image::<RGB16>(2, 2, &[32768u16; 12]).unwrap();
        drop(enc);
        let t = decode(&tif).unwrap();
        assert_eq!(t.transfer, Transfer::Gamma22Assumed);
        assert_eq!(t.white_level, [1.0; 3]);
        assert!(t.camera.is_none());

        let dng = dir.path("frame.dng");
        DngSpec {
            width: 8,
            height: 8,
            black: 0,
            white: 4095,
            pattern: "RGGB",
            make: "Negg",
            model: "Synthetic",
            matrix: None,
            active_area: None,
            data: mosaic_from_scene(8, 8, "RGGB", |_, _| [1000, 2000, 3000]),
        }
        .write(&dng);
        let r = decode(&dng).unwrap();
        assert_eq!(r.transfer, Transfer::Linear);
        assert_eq!(r.white_level, [4095.0; 3]);
        let p = r.image.pixel(3, 3);
        assert!((p[1] - 2000.0 / 4095.0).abs() < 1e-6, "{p:?}");

        // A DNG named .tif goes down the TIFF path and fails there: CFA
        // data is not RGB, and there is no retry through rawler.
        let misnamed = dir.path("frame.tif");
        std::fs::copy(&dng, &misnamed).unwrap();
        match decode(&misnamed) {
            Err(DecodeError::Unsupported(_)) | Err(DecodeError::Tiff(_)) => {}
            other => panic!("expected a TIFF-path error, got {other:?}"),
        }
    }
}
