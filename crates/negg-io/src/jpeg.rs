// SPDX-License-Identifier: MPL-2.0

//! 8-bit RGB JPEG writer.
//!
//! The pixel data arrives already quantised to `u8` (ground rule 4 of the
//! plan). ADR 0010 fixes quality 92 and the encoder's default chroma
//! subsampling. The ICC profile goes into APP2 segments, which is what
//! `image`'s `JpegEncoder::set_icc_profile` writes.

use std::path::Path;

use image::ImageEncoder;
use image::codecs::jpeg::JpegEncoder;
use image::error::{ImageError, UnsupportedError};

use crate::error::{IoError, check_rgb_dimensions};
use crate::sidecar::write_atomic;

/// The quality ADR 0010 fixes for JPEG exports.
pub const DEFAULT_JPEG_QUALITY: u8 = 92;

/// Options for [`write_jpeg8`] and [`encode_jpeg8`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JpegOptions {
    /// ICC profile bytes to embed in APP2 segments, if any.
    pub icc: Option<Vec<u8>>,
    /// Encoder quality, 1 to 100.
    pub quality: u8,
}

impl Default for JpegOptions {
    fn default() -> Self {
        JpegOptions {
            icc: None,
            quality: DEFAULT_JPEG_QUALITY,
        }
    }
}

/// Encodes an interleaved 8-bit RGB buffer as JPEG bytes in memory.
///
/// `rgb` holds `width * height * 3` samples in row-major order, red first.
/// The bytes are identical to those [`write_jpeg8`] writes to a file.
///
/// # Errors
///
/// [`IoError::Dimensions`] when the buffer does not match the dimensions or
/// a side exceeds 65535 pixels, [`IoError::InvalidOption`] when `quality`
/// is outside 1 to 100 or the ICC profile is empty, and [`IoError::Jpeg`]
/// when the encoder fails (including an ICC profile too large for APP2
/// segments).
pub fn encode_jpeg8(
    width: usize,
    height: usize,
    rgb: &[u8],
    opts: &JpegOptions,
) -> Result<Vec<u8>, IoError> {
    let (w, h) = check_rgb_dimensions(width, height, rgb.len())?;
    if w > u32::from(u16::MAX) || h > u32::from(u16::MAX) {
        return Err(IoError::Dimensions {
            width,
            height,
            len: rgb.len(),
        });
    }
    if !(1..=100).contains(&opts.quality) {
        return Err(IoError::InvalidOption(format!(
            "JPEG quality must be between 1 and 100, got {}",
            opts.quality
        )));
    }
    if matches!(&opts.icc, Some(icc) if icc.is_empty()) {
        return Err(IoError::InvalidOption(
            "an ICC profile must not be empty; use None to omit it".into(),
        ));
    }

    let mut out = Vec::new();
    {
        let mut encoder = JpegEncoder::new_with_quality(&mut out, opts.quality);
        if let Some(icc) = &opts.icc {
            encoder
                .set_icc_profile(icc.clone())
                .map_err(unsupported_to_io)?;
        }
        encoder.write_image(rgb, w, h, image::ExtendedColorType::Rgb8)?;
    }
    Ok(out)
}

/// Writes an interleaved 8-bit RGB buffer as a JPEG file.
///
/// `rgb8` holds `width * height * 3` samples in row-major order, red first.
/// The bytes come from [`encode_jpeg8`] and are written atomically (a
/// temporary file beside `path`, synced, then renamed), so a failure leaves
/// any existing file at `path` untouched.
///
/// # Errors
///
/// As [`encode_jpeg8`], plus [`IoError::Io`] for file errors.
pub fn write_jpeg8(
    path: &Path,
    width: usize,
    height: usize,
    rgb8: &[u8],
    opts: &JpegOptions,
) -> Result<(), IoError> {
    let bytes = encode_jpeg8(width, height, rgb8, opts)?;
    write_atomic(path, &bytes).map(|_| ())
}

fn unsupported_to_io(e: UnsupportedError) -> IoError {
    IoError::Jpeg(ImageError::Unsupported(e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageDecoder;
    use image::codecs::jpeg::JpegDecoder;
    use std::fs::{self, File};
    use std::io::BufReader;

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "negg-io-jpeg-{name}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }

        fn path(&self, name: &str) -> std::path::PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn gradient(width: usize, height: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(width * height * 3);
        for y in 0..height {
            for x in 0..width {
                out.push((x * 255 / width.max(1)) as u8);
                out.push((y * 255 / height.max(1)) as u8);
                out.push(128);
            }
        }
        out
    }

    #[test]
    fn writes_decodable_jpeg_with_icc() {
        let dir = TempDir::new("icc");
        let path = dir.path("out.jpg");
        let (w, h) = (16, 12);
        let icc: Vec<u8> = (0..600u32).map(|i| (i * 7 % 256) as u8).collect();
        let opts = JpegOptions {
            icc: Some(icc.clone()),
            quality: 92,
        };
        write_jpeg8(&path, w, h, &gradient(w, h), &opts).unwrap();

        let mut decoder = JpegDecoder::new(BufReader::new(File::open(&path).unwrap())).unwrap();
        assert_eq!(decoder.dimensions(), (16, 12));
        assert_eq!(decoder.color_type(), image::ColorType::Rgb8);
        assert_eq!(decoder.icc_profile().unwrap(), Some(icc));
        let mut pixels = vec![0u8; decoder.total_bytes() as usize];
        decoder.read_image(&mut pixels).unwrap();
        assert_eq!(pixels.len(), w * h * 3);
        // Lossy, but the blue channel is a flat 128 so it survives within a
        // small tolerance.
        let blue_mean = pixels.chunks(3).map(|p| p[2] as u32).sum::<u32>() / (w * h) as u32;
        assert!((120..=136).contains(&blue_mean), "blue mean {blue_mean}");
    }

    #[test]
    fn no_icc_means_no_profile() {
        let dir = TempDir::new("noicc");
        let path = dir.path("plain.jpg");
        write_jpeg8(&path, 4, 4, &gradient(4, 4), &JpegOptions::default()).unwrap();
        let mut decoder = JpegDecoder::new(BufReader::new(File::open(&path).unwrap())).unwrap();
        assert_eq!(decoder.icc_profile().unwrap(), None);
    }

    #[test]
    fn large_icc_is_split_across_segments_and_reassembled() {
        let dir = TempDir::new("bigicc");
        let path = dir.path("big.jpg");
        // Larger than one APP2 segment (65533 bytes of payload).
        let icc: Vec<u8> = (0..150_000u32).map(|i| (i % 256) as u8).collect();
        let opts = JpegOptions {
            icc: Some(icc.clone()),
            quality: 50,
        };
        write_jpeg8(&path, 4, 4, &gradient(4, 4), &opts).unwrap();
        let mut decoder = JpegDecoder::new(BufReader::new(File::open(&path).unwrap())).unwrap();
        assert_eq!(decoder.icc_profile().unwrap(), Some(icc));
    }

    #[test]
    fn encode_equals_the_file_write_jpeg8_writes() {
        let dir = TempDir::new("encode");
        let path = dir.path("e.jpg");
        let (w, h) = (23, 17);
        let opts = JpegOptions {
            icc: Some((0..300u32).map(|i| (i % 251) as u8).collect()),
            quality: 85,
        };
        let pixels = gradient(w, h);
        write_jpeg8(&path, w, h, &pixels, &opts).unwrap();
        let bytes = encode_jpeg8(w, h, &pixels, &opts).unwrap();
        assert_eq!(bytes, fs::read(&path).unwrap());
        assert!(!dir.path("e.jpg.tmp").exists());
        assert!(matches!(
            encode_jpeg8(w, h + 1, &pixels, &opts),
            Err(IoError::Dimensions { .. })
        ));
    }

    #[test]
    fn rejects_bad_dimensions_and_options() {
        let dir = TempDir::new("bad");
        let path = dir.path("bad.jpg");
        let pixels = gradient(2, 2);
        assert!(matches!(
            write_jpeg8(&path, 2, 3, &pixels, &JpegOptions::default()),
            Err(IoError::Dimensions { .. })
        ));
        for quality in [0u8, 101] {
            let opts = JpegOptions { icc: None, quality };
            assert!(matches!(
                write_jpeg8(&path, 2, 2, &pixels, &opts),
                Err(IoError::InvalidOption(_))
            ));
        }
        let empty_icc = JpegOptions {
            icc: Some(Vec::new()),
            quality: 92,
        };
        assert!(matches!(
            write_jpeg8(&path, 2, 2, &pixels, &empty_icc),
            Err(IoError::InvalidOption(_))
        ));
        assert!(
            !path.exists(),
            "validation must happen before the file is created"
        );
    }
}
