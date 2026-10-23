// SPDX-License-Identifier: MPL-2.0

//! 16-bit RGB TIFF writer.
//!
//! The pixel data arrives already quantised to `u16` (ground rule 4 of the
//! plan: quantisation happens exactly once, in the exporter, through
//! negg-cms). This module only lays the samples out in a file and attaches
//! the tags the plan asks for: Orientation 1, X/YResolution with
//! ResolutionUnit inch (ADR 0010: 300 pixels per inch by default), and the
//! ICC profile in tag 34675 as UNDEFINED bytes when one is given.
//!
//! The image is cut into strips of [`ROWS_PER_STRIP`] rows. Each strip is
//! compressed on its own, in parallel with rayon, with the `tiff` crate's
//! own compressors (Deflate at its fast level by default, ADR 0013), and
//! the compressed strips are then written in order through the crate's
//! low-level directory encoder, which also writes the tags.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use rayon::prelude::*;
use tiff::Directory;
use tiff::encoder::compression::{CompressionAlgorithm, Deflate, DeflateLevel, Lzw, Packbits};
use tiff::encoder::{Rational, TiffEncoder};
use tiff::tags::{CompressionMethod, PhotometricInterpretation, ResolutionUnit, Tag, Type};

use crate::error::{IoError, check_rgb_dimensions, io_at};

/// Rows per TIFF strip: about 2.3 MB of samples for a 6000-pixel-wide
/// frame, so a 24 MP image gives about 60 strips to compress in parallel.
pub const ROWS_PER_STRIP: usize = 64;

/// Compression scheme for the TIFF strips.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Compression {
    /// No compression.
    None,
    /// Deflate (zlib) at the fast level, the default: lossless and widely
    /// read. ADR 0013 chose the fast level for the Gate 1 timing.
    #[default]
    Deflate,
    /// LZW.
    Lzw,
    /// PackBits run-length encoding (each row packed separately, as the
    /// TIFF specification requires).
    PackBits,
}

impl Compression {
    fn method(self) -> CompressionMethod {
        match self {
            Compression::None => CompressionMethod::None,
            Compression::Deflate => CompressionMethod::Deflate,
            Compression::Lzw => CompressionMethod::LZW,
            Compression::PackBits => CompressionMethod::PackBits,
        }
    }

    /// Compresses one strip of `row_bytes`-byte rows.
    fn compress(self, strip: &[u8], row_bytes: usize) -> std::io::Result<Vec<u8>> {
        let mut out = Vec::new();
        match self {
            Compression::None => out.extend_from_slice(strip),
            Compression::Deflate => {
                Deflate::with_level(DeflateLevel::Fast).write_to(&mut out, strip)?;
            }
            Compression::Lzw => {
                Lzw.write_to(&mut out, strip)?;
            }
            Compression::PackBits => {
                for row in strip.chunks(row_bytes) {
                    Packbits.write_to(&mut out, row)?;
                }
            }
        }
        Ok(out)
    }
}

/// Options for [`write_tiff16`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TiffOptions {
    /// ICC profile bytes to embed in tag 34675, if any.
    pub icc: Option<Vec<u8>>,
    /// Resolution written to XResolution and YResolution, in pixels per
    /// inch. Must be at least 1.
    pub resolution_ppi: u32,
    /// Strip compression.
    pub compression: Compression,
}

/// The resolution ADR 0010 fixes for exports, in pixels per inch.
pub const DEFAULT_RESOLUTION_PPI: u32 = 300;

impl Default for TiffOptions {
    fn default() -> Self {
        TiffOptions {
            icc: None,
            resolution_ppi: DEFAULT_RESOLUTION_PPI,
            compression: Compression::Deflate,
        }
    }
}

/// Writes an interleaved 16-bit RGB buffer as a TIFF file.
///
/// `rgb16` holds `width * height * 3` samples in row-major order, red first.
/// Tags written: ImageWidth/Length, BitsPerSample 16, SamplesPerPixel 3,
/// Photometric RGB, PlanarConfiguration 1 (chunky), SampleFormat unsigned,
/// Compression per `opts`, RowsPerStrip [`ROWS_PER_STRIP`] (or the height,
/// if smaller), StripOffsets and StripByteCounts, Orientation (274) = 1,
/// XResolution/YResolution (282/283) = `resolution_ppi / 1`,
/// ResolutionUnit (296) = 2 (inch), and ICC (34675, type UNDEFINED) when
/// `opts.icc` is `Some`. Samples are in the machine's byte order, which the
/// file header records.
///
/// The strips are compressed in parallel (see the module documentation).
/// The file is written in place (no temporary file); a failure part way
/// through leaves a truncated file at `path`.
///
/// # Errors
///
/// [`IoError::Dimensions`] when the buffer does not match the dimensions,
/// [`IoError::InvalidOption`] when `resolution_ppi` is 0 or the ICC profile
/// is empty, [`IoError::Io`] for file errors and [`IoError::Tiff`] when the
/// encoder fails.
pub fn write_tiff16(
    path: &Path,
    width: usize,
    height: usize,
    rgb16: &[u16],
    opts: &TiffOptions,
) -> Result<(), IoError> {
    let (w, h) = check_rgb_dimensions(width, height, rgb16.len())?;
    if opts.resolution_ppi == 0 {
        return Err(IoError::InvalidOption(
            "TIFF resolution must be at least 1 pixel per inch".into(),
        ));
    }
    if matches!(&opts.icc, Some(icc) if icc.is_empty()) {
        return Err(IoError::InvalidOption(
            "an ICC profile must not be empty; use None to omit it".into(),
        ));
    }

    // Compress every strip in parallel before the file is touched. The
    // samples go to bytes in native order, matching the header the `tiff`
    // encoder writes.
    let row_samples = width * 3;
    let row_bytes = row_samples * 2;
    let rows_per_strip = ROWS_PER_STRIP.min(height);
    let strips: Vec<Vec<u8>> = rgb16
        .par_chunks(row_samples * rows_per_strip)
        .map(|samples| {
            let bytes: Vec<u8> = samples.iter().flat_map(|v| v.to_ne_bytes()).collect();
            opts.compression.compress(&bytes, row_bytes)
        })
        .collect::<std::io::Result<_>>()
        .map_err(io_at(path))?;

    let file = File::create(path).map_err(io_at(path))?;
    let mut writer = BufWriter::new(file);
    {
        let mut encoder = TiffEncoder::new(&mut writer)?;
        let mut directory = encoder.image_directory()?;
        let mut offsets = Vec::with_capacity(strips.len());
        let mut byte_counts = Vec::with_capacity(strips.len());
        for strip in &strips {
            let offset = directory.write_data(&strip[..])?;
            offsets.push(u32::try_from(offset).map_err(|_| too_large(path))?);
            byte_counts.push(u32::try_from(strip.len()).map_err(|_| too_large(path))?);
        }
        drop(strips);
        let resolution = Rational {
            n: opts.resolution_ppi,
            d: 1,
        };
        directory.write_tag(Tag::ImageWidth, w)?;
        directory.write_tag(Tag::ImageLength, h)?;
        directory.write_tag(Tag::BitsPerSample, &[16u16, 16, 16][..])?;
        directory.write_tag(Tag::Compression, opts.compression.method().to_u16())?;
        directory.write_tag(
            Tag::PhotometricInterpretation,
            PhotometricInterpretation::RGB.to_u16(),
        )?;
        directory.write_tag(Tag::StripOffsets, &offsets[..])?;
        directory.write_tag(Tag::Orientation, 1u16)?;
        directory.write_tag(Tag::SamplesPerPixel, 3u16)?;
        directory.write_tag(
            Tag::RowsPerStrip,
            u32::try_from(rows_per_strip).map_err(|_| too_large(path))?,
        )?;
        directory.write_tag(Tag::StripByteCounts, &byte_counts[..])?;
        directory.write_tag(Tag::XResolution, resolution.clone())?;
        directory.write_tag(Tag::YResolution, resolution)?;
        directory.write_tag(Tag::PlanarConfiguration, 1u16)?;
        directory.write_tag(Tag::ResolutionUnit, ResolutionUnit::Inch.to_u16())?;
        directory.write_tag(Tag::SampleFormat, &[1u16, 1, 1][..])?;
        if let Some(icc) = &opts.icc {
            // The ICC tag is typed UNDEFINED by the ICC specification. The
            // encoder's typed `write_tag` has no UNDEFINED value type, so the
            // entry is written raw and inserted through a one-entry
            // directory.
            let entry = directory.write_entry_bytes(Type::UNDEFINED, icc)?;
            let mut extra = Directory::empty();
            extra.extend([(Tag::IccProfile, entry)]);
            directory.extend_from(&extra);
        }
        directory.finish()?;
    }
    let file = writer
        .into_inner()
        .map_err(|e| io_at(path)(e.into_error()))?;
    file.sync_all().map_err(io_at(path))?;
    Ok(())
}

/// The error for an image whose compressed data does not fit the 4 GiB
/// offsets of a classic TIFF.
fn too_large(path: &Path) -> IoError {
    io_at(path)(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        "image too large for a classic TIFF (offsets past 4 GiB)",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Cursor;
    use tiff::decoder::{Decoder, DecodingResult, ifd::Value};

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "negg-io-tiff-{name}-{}-{}",
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

    fn ramp(width: usize, height: usize) -> Vec<u16> {
        (0..width * height * 3)
            .map(|i| ((i * 977) % 65536) as u16)
            .collect()
    }

    fn bytes_of(value: Value) -> Vec<u8> {
        match value {
            Value::List(items) => items
                .into_iter()
                .map(|v| match v {
                    Value::Byte(b) => b,
                    other => panic!("unexpected ICC element {other:?}"),
                })
                .collect(),
            Value::Byte(b) => vec![b],
            other => panic!("unexpected ICC value {other:?}"),
        }
    }

    #[test]
    fn round_trip_with_icc_and_tags() {
        let dir = TempDir::new("icc");
        let path = dir.path("out.tif");
        let (w, h) = (7, 5);
        let pixels = ramp(w, h);
        let icc: Vec<u8> = (0..300u32).map(|i| (i % 251) as u8).collect();
        let opts = TiffOptions {
            icc: Some(icc.clone()),
            ..TiffOptions::default()
        };
        write_tiff16(&path, w, h, &pixels, &opts).unwrap();

        let bytes = fs::read(&path).unwrap();
        let mut decoder = Decoder::new(Cursor::new(bytes)).unwrap();
        assert_eq!(decoder.dimensions().unwrap(), (7, 5));
        assert_eq!(
            decoder.colortype().unwrap(),
            tiff::ColorType::RGB(16),
            "16-bit RGB"
        );
        assert_eq!(decoder.get_tag_u32(Tag::Orientation).unwrap(), 1);
        assert_eq!(
            decoder.get_tag(Tag::XResolution).unwrap(),
            Value::Rational(300, 1)
        );
        assert_eq!(
            decoder.get_tag(Tag::YResolution).unwrap(),
            Value::Rational(300, 1)
        );
        assert_eq!(decoder.get_tag_u32(Tag::ResolutionUnit).unwrap(), 2);
        assert_eq!(
            decoder.get_tag_u32(Tag::Compression).unwrap(),
            tiff::tags::CompressionMethod::Deflate.to_u16() as u32
        );
        assert_eq!(bytes_of(decoder.get_tag(Tag::IccProfile).unwrap()), icc);
        match decoder.read_image().unwrap() {
            DecodingResult::U16(data) => assert_eq!(data, pixels),
            other => panic!("expected U16 data, got {other:?}"),
        }
    }

    #[test]
    fn no_icc_means_no_tag() {
        let dir = TempDir::new("noicc");
        let path = dir.path("plain.tif");
        let pixels = ramp(3, 2);
        write_tiff16(&path, 3, 2, &pixels, &TiffOptions::default()).unwrap();
        let mut decoder = Decoder::new(Cursor::new(fs::read(&path).unwrap())).unwrap();
        assert!(decoder.find_tag(Tag::IccProfile).unwrap().is_none());
        assert_eq!(decoder.get_tag_u32(Tag::Orientation).unwrap(), 1);
    }

    #[test]
    fn every_compression_round_trips() {
        let dir = TempDir::new("compression");
        // 140 rows: two full strips and a short last one.
        let (w, h) = (9, 2 * ROWS_PER_STRIP + 12);
        let pixels = ramp(w, h);
        for (i, compression) in [
            Compression::None,
            Compression::Deflate,
            Compression::Lzw,
            Compression::PackBits,
        ]
        .into_iter()
        .enumerate()
        {
            let path = dir.path(&format!("c{i}.tif"));
            let opts = TiffOptions {
                compression,
                resolution_ppi: 72,
                icc: None,
            };
            write_tiff16(&path, w, h, &pixels, &opts).unwrap();
            let mut decoder = Decoder::new(Cursor::new(fs::read(&path).unwrap())).unwrap();
            assert_eq!(
                decoder.get_tag(Tag::XResolution).unwrap(),
                Value::Rational(72, 1)
            );
            assert_eq!(
                decoder.get_tag_u32(Tag::RowsPerStrip).unwrap(),
                ROWS_PER_STRIP as u32
            );
            assert_eq!(
                decoder.get_tag_u32_vec(Tag::StripOffsets).unwrap().len(),
                3,
                "{compression:?}"
            );
            match decoder.read_image().unwrap() {
                DecodingResult::U16(data) => assert_eq!(data, pixels, "{compression:?}"),
                other => panic!("expected U16 data, got {other:?}"),
            }
        }
    }

    #[test]
    fn rejects_bad_dimensions_and_options() {
        let dir = TempDir::new("bad");
        let path = dir.path("bad.tif");
        let pixels = ramp(2, 2);
        assert!(matches!(
            write_tiff16(&path, 2, 3, &pixels, &TiffOptions::default()),
            Err(IoError::Dimensions { .. })
        ));
        assert!(matches!(
            write_tiff16(&path, 0, 0, &[], &TiffOptions::default()),
            Err(IoError::Dimensions { .. })
        ));
        let zero_ppi = TiffOptions {
            resolution_ppi: 0,
            ..TiffOptions::default()
        };
        assert!(matches!(
            write_tiff16(&path, 2, 2, &pixels, &zero_ppi),
            Err(IoError::InvalidOption(_))
        ));
        let empty_icc = TiffOptions {
            icc: Some(Vec::new()),
            ..TiffOptions::default()
        };
        assert!(matches!(
            write_tiff16(&path, 2, 2, &pixels, &empty_icc),
            Err(IoError::InvalidOption(_))
        ));
        assert!(
            !path.exists(),
            "validation must happen before the file is created"
        );
    }

    #[test]
    fn missing_directory_is_an_io_error() {
        let dir = TempDir::new("missing");
        let path = dir.path("no/such/dir/out.tif");
        let pixels = ramp(2, 2);
        match write_tiff16(&path, 2, 2, &pixels, &TiffOptions::default()) {
            Err(IoError::Io { path: p, .. }) => assert_eq!(p, path),
            other => panic!("expected Io error, got {other:?}"),
        }
    }
}
