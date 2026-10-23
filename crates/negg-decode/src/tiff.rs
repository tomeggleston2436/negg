// SPDX-License-Identifier: MPL-2.0

//! Reading RGB TIFF files into an [`Image`].
//!
//! [`read_tiff`] is the phase 0 reader for the reference renders in the
//! corpus: 8-bit and 16-bit chunky RGB or RGBA (alpha dropped), any
//! compression the `tiff` crate decodes, plus 32-bit IEEE float RGB and
//! RGBA. Integer sample values are normalised to `[0, 1]` by dividing by
//! the sample maximum and nothing else, so the result holds the file's
//! *encoded* values and an sRGB TIFF stays sRGB-encoded. Float samples are
//! returned as stored. No linearisation happens here; that is a
//! colour-management decision made by the caller.
//!
//! [`decode_tiff`] wraps the same reader in the [`Decoded`] contract and
//! says, through [`Transfer`], how the values are encoded: the embedded
//! ICC profile when there is one, linear for float samples, and otherwise
//! an assumed gamma 2.2 with a warning. 16-bit input is never reduced to
//! 8 bits: everything is `f32` from here on.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use negg_types::Image;
use negg_types::image::CHANNELS;
use tiff::decoder::{Decoder, DecodingResult, Limits};
use tiff::tags::{PlanarConfiguration, SampleFormat, Tag};

use crate::error::DecodeError;
use crate::{Decoded, Transfer};

/// A decoded TIFF: the pixels plus the metadata the caller needs to
/// interpret them.
#[derive(Clone, Debug)]
pub struct TiffImage {
    /// The pixels as encoded values, interleaved RGB. Integer samples are
    /// scaled to `[0, 1]`; float samples are as stored.
    pub image: Image,
    /// Bits per sample in the file: 8, 16 or 32 (float only).
    pub bits_per_sample: u16,
    /// The embedded ICC profile (tag 34675), raw bytes, if the file has one.
    pub icc_profile: Option<Vec<u8>>,
    /// Samples per pixel in the file: 3 for RGB, 4 for RGBA.
    pub samples_per_pixel: u16,
    /// Whether the samples are IEEE floating point (tag 339 = 3). Float
    /// TIFFs are taken to be linear.
    pub float: bool,
}

/// Reads the first image of a TIFF file.
///
/// Accepts chunky RGB (3 samples) and RGBA (4 samples, alpha discarded) at
/// 8 or 16 bits per unsigned sample or 32 bits per float sample. Anything
/// else, including greyscale, palette, planar layouts and signed integers,
/// returns [`DecodeError::Unsupported`]. Decoder limits are lifted, since
/// corpus files are trusted and a 16-bit RGBA scan easily exceeds the
/// `tiff` crate's default buffer cap.
///
/// # Errors
///
/// [`DecodeError::Io`] if the file cannot be opened or read,
/// [`DecodeError::Tiff`] if the `tiff` crate rejects it,
/// [`DecodeError::Unsupported`] for layouts this reader does not handle.
pub fn read_tiff(path: &Path) -> Result<TiffImage, DecodeError> {
    let file = File::open(path)?;
    let mut decoder = Decoder::new(BufReader::new(file))?.with_limits(Limits::unlimited());

    let (width, height) = decoder.dimensions()?;
    let colour = decoder.colortype()?;
    let (bits_per_sample, samples_per_pixel) = match colour {
        tiff::ColorType::RGB(bits) => (bits, 3u16),
        tiff::ColorType::RGBA(bits) => (bits, 4u16),
        other => {
            return Err(DecodeError::Unsupported(format!(
                "colour type {other:?}; only RGB and RGBA are read"
            )));
        }
    };

    let float = sample_format_is_float(&mut decoder)?;
    match (bits_per_sample, float) {
        (8, false) | (16, false) | (32, true) => {}
        (bits, false) => {
            return Err(DecodeError::Unsupported(format!(
                "{bits} bits per sample; only 8 and 16-bit integer samples are read"
            )));
        }
        (bits, true) => {
            return Err(DecodeError::Unsupported(format!(
                "{bits}-bit float samples; only 32-bit float samples are read"
            )));
        }
    }

    let planar = decoder
        .find_tag(Tag::PlanarConfiguration)?
        .map(|v| v.into_u16())
        .transpose()?
        .map(PlanarConfiguration::from_u16);
    if let Some(Some(PlanarConfiguration::Planar)) = planar {
        return Err(DecodeError::Unsupported(
            "planar sample layout; only chunky (interleaved) TIFFs are read".into(),
        ));
    }
    if let Some(None) = planar {
        return Err(DecodeError::Unsupported(
            "unknown planar configuration value".into(),
        ));
    }

    let icc_profile = decoder
        .find_tag(Tag::IccProfile)?
        .map(|v| v.into_u8_vec())
        .transpose()?;

    let samples = usize::from(samples_per_pixel);
    let data: Vec<f32> = match decoder.read_image()? {
        DecodingResult::U8(buf) => normalise(&buf, samples, f32::from(u8::MAX)),
        DecodingResult::U16(buf) => normalise(&buf, samples, f32::from(u16::MAX)),
        DecodingResult::F32(buf) => normalise(&buf, samples, 1.0),
        other => {
            return Err(DecodeError::Unsupported(format!(
                "sample format {}; only unsigned 8 and 16-bit and float 32-bit samples are read",
                sample_name(&other)
            )));
        }
    };

    let image = Image::from_vec(width as usize, height as usize, data)?;
    Ok(TiffImage {
        image,
        bits_per_sample: u16::from(bits_per_sample),
        icc_profile,
        samples_per_pixel,
        float,
    })
}

/// Decodes a TIFF into the [`Decoded`] contract.
///
/// The image values are returned exactly as [`read_tiff`] reads them. The
/// transfer is [`Transfer::Icc`] with the profile bytes when the file has
/// one, [`Transfer::Linear`] for float samples, and otherwise
/// [`Transfer::Gamma22Assumed`] with the warning "no ICC profile and
/// integer samples: assuming gamma 2.2". No linearisation is applied here.
/// Levels are reported as black 0 and white 1, since the file carries no
/// sensor calibration.
///
/// # Errors
///
/// Whatever [`read_tiff`] returns.
pub fn decode_tiff(path: &Path) -> Result<Decoded, DecodeError> {
    let t = read_tiff(path)?;
    let mut warnings = Vec::new();
    let transfer = match (t.icc_profile, t.float) {
        (Some(icc), _) => Transfer::Icc(icc),
        (None, true) => Transfer::Linear,
        (None, false) => {
            warnings.push("no ICC profile and integer samples: assuming gamma 2.2".into());
            Transfer::Gamma22Assumed
        }
    };
    Ok(Decoded {
        image: t.image,
        transfer,
        xyz_to_camera: None,
        matrix_white: None,
        camera: None,
        white_level: [1.0; CHANNELS],
        black_level: [0.0; CHANNELS],
        warnings,
    })
}

/// Reads the SampleFormat tag: `true` for IEEE float, `false` for unsigned
/// integer or absent.
///
/// # Errors
///
/// [`DecodeError::Unsupported`] for signed integers, mixed formats and
/// unknown values.
fn sample_format_is_float<R: std::io::Read + std::io::Seek>(
    decoder: &mut Decoder<R>,
) -> Result<bool, DecodeError> {
    let Some(value) = decoder.find_tag(Tag::SampleFormat)? else {
        return Ok(false);
    };
    let formats: Vec<SampleFormat> = value
        .into_u16_vec()?
        .into_iter()
        .map(SampleFormat::from_u16_exhaustive)
        .collect();
    let Some(first) = formats.first() else {
        return Ok(false);
    };
    if formats.iter().any(|f| f != first) {
        return Err(DecodeError::Unsupported(format!(
            "mixed sample formats {formats:?}"
        )));
    }
    match first {
        SampleFormat::Uint => Ok(false),
        SampleFormat::IEEEFP => Ok(true),
        other => Err(DecodeError::Unsupported(format!(
            "sample format {other:?}; only unsigned integer and IEEE float are read"
        ))),
    }
}

/// Converts interleaved samples to `f32`, dividing by `max`, keeping the
/// first three of every `samples` and dropping the rest (alpha).
fn normalise<T>(buf: &[T], samples: usize, max: f32) -> Vec<f32>
where
    T: Copy + Into<f32>,
{
    let mut out = Vec::with_capacity(buf.len() / samples * CHANNELS);
    for px in buf.chunks_exact(samples) {
        for &s in &px[..CHANNELS] {
            out.push(s.into() / max);
        }
    }
    out
}

/// A short name for a decoding result variant, for error messages.
fn sample_name(r: &DecodingResult) -> &'static str {
    match r {
        DecodingResult::U8(_) => "u8",
        DecodingResult::U16(_) => "u16",
        DecodingResult::U32(_) => "u32",
        DecodingResult::U64(_) => "u64",
        DecodingResult::F16(_) => "f16",
        DecodingResult::F32(_) => "f32",
        DecodingResult::F64(_) => "f64",
        DecodingResult::I8(_) => "i8",
        DecodingResult::I16(_) => "i16",
        DecodingResult::I32(_) => "i32",
        DecodingResult::I64(_) => "i64",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;
    use tiff::encoder::TiffEncoder;
    use tiff::encoder::colortype::{Gray8, RGB8, RGB16, RGB32, RGB32Float, RGBA8, RGBA32Float};

    #[test]
    fn reads_8_bit_rgb() {
        let dir = TempDir::new("rgb8");
        let path = dir.path("rgb8.tif");
        let (w, h) = (5u32, 3u32);
        let mut data = vec![0u8; (w * h * 3) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 3) as usize;
                data[i] = (x * 40) as u8;
                data[i + 1] = (y * 100) as u8;
                data[i + 2] = 255;
            }
        }
        let mut enc = TiffEncoder::new(File::create(&path).unwrap()).unwrap();
        enc.write_image::<RGB8>(w, h, &data).unwrap();
        drop(enc);

        let t = read_tiff(&path).unwrap();
        assert_eq!(t.image.width(), 5);
        assert_eq!(t.image.height(), 3);
        assert_eq!(t.bits_per_sample, 8);
        assert_eq!(t.samples_per_pixel, 3);
        assert!(!t.float);
        assert!(t.icc_profile.is_none());
        let p = t.image.pixel(3, 2);
        assert_eq!(p, [120.0 / 255.0, 200.0 / 255.0, 1.0]);
        assert_eq!(t.image.pixel(0, 0), [0.0, 0.0, 1.0]);
    }

    #[test]
    fn reads_16_bit_rgb() {
        let dir = TempDir::new("rgb16");
        let path = dir.path("rgb16.tif");
        let (w, h) = (4u32, 4u32);
        let mut data = vec![0u16; (w * h * 3) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 3) as usize;
                data[i] = (x * 10_000) as u16;
                data[i + 1] = (y * 10_000) as u16;
                data[i + 2] = 65_535;
            }
        }
        let mut enc = TiffEncoder::new(File::create(&path).unwrap()).unwrap();
        enc.write_image::<RGB16>(w, h, &data).unwrap();
        drop(enc);

        let t = read_tiff(&path).unwrap();
        assert_eq!((t.image.width(), t.image.height()), (4, 4));
        assert_eq!(t.bits_per_sample, 16);
        assert_eq!(t.samples_per_pixel, 3);
        let p = t.image.pixel(2, 1);
        assert_eq!(p, [20_000.0 / 65_535.0, 10_000.0 / 65_535.0, 1.0]);
    }

    #[test]
    fn reads_rgba_and_drops_alpha() {
        let dir = TempDir::new("rgba8");
        let path = dir.path("rgba8.tif");
        let data: Vec<u8> = vec![10, 20, 30, 40, 50, 60, 70, 80];
        let mut enc = TiffEncoder::new(File::create(&path).unwrap()).unwrap();
        enc.write_image::<RGBA8>(2, 1, &data).unwrap();
        drop(enc);

        let t = read_tiff(&path).unwrap();
        assert_eq!(t.samples_per_pixel, 4);
        assert_eq!(t.image.width(), 2);
        assert_eq!(
            t.image.pixel(1, 0),
            [50.0 / 255.0, 60.0 / 255.0, 70.0 / 255.0]
        );
    }

    #[test]
    fn reads_32_bit_float_rgb_as_stored() {
        let dir = TempDir::new("rgbf32");
        let path = dir.path("rgbf32.tif");
        let data: Vec<f32> = vec![0.0, 0.5, 1.0, 1.5, -0.25, 0.125];
        let mut enc = TiffEncoder::new(File::create(&path).unwrap()).unwrap();
        enc.write_image::<RGB32Float>(2, 1, &data).unwrap();
        drop(enc);

        let t = read_tiff(&path).unwrap();
        assert!(t.float);
        assert_eq!(t.bits_per_sample, 32);
        assert_eq!(t.image.pixel(0, 0), [0.0, 0.5, 1.0]);
        assert_eq!(t.image.pixel(1, 0), [1.5, -0.25, 0.125]);

        let rgba = dir.path("rgbaf32.tif");
        let mut enc = TiffEncoder::new(File::create(&rgba).unwrap()).unwrap();
        enc.write_image::<RGBA32Float>(1, 1, &[0.1, 0.2, 0.3, 0.9])
            .unwrap();
        drop(enc);
        let t = read_tiff(&rgba).unwrap();
        assert_eq!(t.samples_per_pixel, 4);
        assert_eq!(t.image.pixel(0, 0), [0.1, 0.2, 0.3]);
    }

    #[test]
    fn reads_icc_profile_tag() {
        let dir = TempDir::new("icc");
        let path = dir.path("icc.tif");
        let profile: Vec<u8> = (0u8..64).collect();
        let mut enc = TiffEncoder::new(File::create(&path).unwrap()).unwrap();
        let mut img = enc.new_image::<RGB8>(1, 1).unwrap();
        img.encoder()
            .write_tag(Tag::IccProfile, profile.as_slice())
            .unwrap();
        img.write_data(&[1u8, 2, 3]).unwrap();
        drop(enc);

        let t = read_tiff(&path).unwrap();
        assert_eq!(t.icc_profile.as_deref(), Some(profile.as_slice()));
    }

    #[test]
    fn grey_is_unsupported() {
        let dir = TempDir::new("grey");
        let path = dir.path("grey.tif");
        let mut enc = TiffEncoder::new(File::create(&path).unwrap()).unwrap();
        enc.write_image::<Gray8>(2, 2, &[0, 1, 2, 3]).unwrap();
        drop(enc);

        match read_tiff(&path) {
            Err(DecodeError::Unsupported(msg)) => assert!(msg.contains("Gray"), "{msg}"),
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn integer_32_bit_is_unsupported() {
        let dir = TempDir::new("rgb32");
        let path = dir.path("rgb32.tif");
        let mut enc = TiffEncoder::new(File::create(&path).unwrap()).unwrap();
        enc.write_image::<RGB32>(1, 1, &[1u32, 2, 3]).unwrap();
        drop(enc);
        match read_tiff(&path) {
            Err(DecodeError::Unsupported(msg)) => assert!(msg.contains("32 bits"), "{msg}"),
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn missing_file_is_io() {
        let dir = TempDir::new("missing");
        match read_tiff(&dir.path("nope.tif")) {
            Err(DecodeError::Io(_)) => {}
            other => panic!("expected Io, got {other:?}"),
        }
    }

    #[test]
    fn garbage_is_tiff_error() {
        let dir = TempDir::new("garbage");
        let path = dir.path("garbage.tif");
        std::fs::write(&path, b"not a tiff at all").unwrap();
        match read_tiff(&path) {
            Err(DecodeError::Tiff(_)) => {}
            other => panic!("expected Tiff, got {other:?}"),
        }
    }

    #[test]
    fn decode_tiff_reports_the_transfer() {
        let dir = TempDir::new("transfer");

        let float = dir.path("float.tif");
        let mut enc = TiffEncoder::new(File::create(&float).unwrap()).unwrap();
        enc.write_image::<RGB32Float>(1, 1, &[0.25, 0.5, 2.0])
            .unwrap();
        drop(enc);
        let d = decode_tiff(&float).unwrap();
        assert_eq!(d.transfer, Transfer::Linear);
        assert!(d.warnings.is_empty());
        assert_eq!(d.image.pixel(0, 0), [0.25, 0.5, 2.0]);
        assert_eq!(d.white_level, [1.0; 3]);
        assert_eq!(d.black_level, [0.0; 3]);
        assert!(d.xyz_to_camera.is_none() && d.matrix_white.is_none() && d.camera.is_none());

        let int16 = dir.path("int16.tif");
        let mut enc = TiffEncoder::new(File::create(&int16).unwrap()).unwrap();
        enc.write_image::<RGB16>(1, 1, &[65535u16, 0, 32768])
            .unwrap();
        drop(enc);
        let d = decode_tiff(&int16).unwrap();
        assert_eq!(d.transfer, Transfer::Gamma22Assumed);
        assert_eq!(
            d.warnings,
            vec!["no ICC profile and integer samples: assuming gamma 2.2".to_string()]
        );
        assert_eq!(d.image.pixel(0, 0)[0], 1.0);

        let icc = dir.path("icc.tif");
        let profile: Vec<u8> = (0u8..32).collect();
        let mut enc = TiffEncoder::new(File::create(&icc).unwrap()).unwrap();
        let mut img = enc.new_image::<RGB16>(1, 1).unwrap();
        img.encoder()
            .write_tag(Tag::IccProfile, profile.as_slice())
            .unwrap();
        img.write_data(&[1u16, 2, 3]).unwrap();
        drop(enc);
        let d = decode_tiff(&icc).unwrap();
        assert_eq!(d.transfer, Transfer::Icc(profile));
        assert!(d.warnings.is_empty());
    }
}
