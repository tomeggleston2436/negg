// SPDX-License-Identifier: MPL-2.0

//! Little-CMS transforms between the working space and everything else
//! (step 7 of `docs/MATHS.md`): export to an output profile as 16-bit or
//! 8-bit encoded RGB, and the step 1 TIFF paths that undo an embedded
//! matrix/TRC ICC profile's TRC in its own primaries (ADR 0012) or an
//! assumed gamma 2.2 encoding.
//!
//! # Threading
//!
//! Little-CMS transforms are created with the default (cached) flags, so
//! a Little-CMS transform is `Send` but not `Sync`. [`OutputTransform`]
//! therefore does not share one between threads: [`OutputTransform::apply`]
//! and [`OutputTransform::apply_u8`] split the image into bands of whole
//! pixels (whole rows and more, for a wide frame) and run them in parallel with rayon, building one Little-CMS transform per
//! worker for the same space and intent (ADR 0013). The result is
//! identical to converting every pixel with a single transform, because
//! each pixel is converted on its own. [`Clone`] for `OutputTransform`
//! rebuilds the transforms from the same space and intent.

use lcms2::{
    ColorSpaceSignature, PixelFormat, Pod, Profile, Tag, TagSignature, ToneCurve, Transform,
};
use negg_types::Image;
use negg_types::settings::OutputSpace;
use rayon::prelude::*;

use crate::error::{CmsError, lcms_error};
use crate::matrix::{Matrix3, icc_colorants_to_rec2020};
use crate::profiles::{output_profile, space_name, working_profile};

/// Pixels per batch handed to Little-CMS. Small enough for the scratch
/// copy to stay in cache, large enough that the per-call overhead is lost.
const CHUNK_PIXELS: usize = 4096;

/// Pixels per parallel band of [`convert`]: whole multiples of
/// [`CHUNK_PIXELS`], so that a 24 MP frame splits into about 90 bands for
/// load balancing while each worker builds its Little-CMS transform once
/// per rayon job rather than once per band.
const BAND_PIXELS: usize = CHUNK_PIXELS * 64;

/// The ICC rendering intent for an export. For the matrix/TRC profiles
/// this crate generates the two are the same transform (matrix/TRC profiles
/// only carry colorimetric data); the choice matters once a LUT-based
/// monitor or printer profile is on the output side.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Intent {
    /// ICC perceptual (the default).
    #[default]
    Perceptual,
    /// ICC media-relative colorimetric.
    RelativeColorimetric,
}

impl Intent {
    fn to_lcms(self) -> lcms2::Intent {
        match self {
            Intent::Perceptual => lcms2::Intent::Perceptual,
            Intent::RelativeColorimetric => lcms2::Intent::RelativeColorimetric,
        }
    }
}

/// A transform from linear Rec.2020 `f32` to encoded integer RGB in one
/// output space. Create it with [`working_to_output`]. `Send` but not
/// `Sync`; see the module documentation.
#[derive(Debug)]
pub struct OutputTransform {
    space: OutputSpace,
    intent: Intent,
    to_u16: Transform<[f32; 3], [u16; 3]>,
    to_u8: Transform<[f32; 3], [u8; 3]>,
}

/// Builds the transform from the working profile to the profile of `space`
/// with the given intent.
///
/// # Errors
///
/// [`CmsError::Lcms`] if the output profile or the transform cannot be
/// built.
pub fn working_to_output(space: OutputSpace, intent: Intent) -> Result<OutputTransform, CmsError> {
    let to_u16 = build_output(space, intent, PixelFormat::RGB_16, "16-bit")?;
    let to_u8 = build_output(space, intent, PixelFormat::RGB_8, "8-bit")?;
    Ok(OutputTransform {
        space,
        intent,
        to_u16,
        to_u8,
    })
}

/// Builds one Little-CMS transform from the working profile to the
/// profile of `space`, with `format` on the output side (`depth` names it
/// in the error).
fn build_output<O: Copy + Pod>(
    space: OutputSpace,
    intent: Intent,
    format: PixelFormat,
    depth: &str,
) -> Result<Transform<[f32; 3], O>, CmsError> {
    let input = working_profile();
    let output = output_profile(space)?;
    let name = space_name(space);
    Transform::new(
        &input,
        PixelFormat::RGB_FLT,
        &output,
        format,
        intent.to_lcms(),
    )
    .map_err(|e| lcms_error(&format!("building the {name} {depth} transform"), e))
}

/// Runs `transform` over the pixels of `src` (interleaved `[f32]`, one
/// pixel per `channels` floats) into `dst`, copying the input in batches
/// of [`CHUNK_PIXELS`] into a `[f32; 3]` scratch buffer (the `Image`
/// buffer is a flat `[f32]`, and Little-CMS wants one array per pixel).
fn convert_band<O: Copy + Pod>(
    transform: &Transform<[f32; 3], O>,
    src: &[f32],
    channels: usize,
    dst: &mut [O],
    scratch: &mut Vec<[f32; 3]>,
) {
    for (src, dst) in src
        .chunks(CHUNK_PIXELS * channels)
        .zip(dst.chunks_mut(CHUNK_PIXELS))
    {
        scratch.clear();
        scratch.extend(src.chunks_exact(channels).map(|p| [p[0], p[1], p[2]]));
        transform.transform_pixels(scratch, dst);
    }
}

/// Converts every pixel of `image` in parallel over bands of
/// [`BAND_PIXELS`]. The first band is converted with `transform`; every
/// other rayon worker builds its own transform with `build` (Little-CMS
/// transforms are not `Sync`).
fn convert<O, B>(transform: &Transform<[f32; 3], O>, build: B, image: &Image) -> Vec<O>
where
    O: Copy + Pod + Send,
    B: Fn() -> Transform<[f32; 3], O> + Sync,
{
    let pixels = image.width() * image.height();
    let mut out = vec![O::zeroed(); pixels];
    let channels = image.channels();
    if pixels <= BAND_PIXELS {
        let mut scratch = Vec::with_capacity(CHUNK_PIXELS.min(pixels));
        convert_band(transform, image.data(), channels, &mut out, &mut scratch);
        return out;
    }
    image
        .data()
        .par_chunks(BAND_PIXELS * channels)
        .zip(out.par_chunks_mut(BAND_PIXELS))
        .for_each_init(
            || (build(), Vec::with_capacity(CHUNK_PIXELS)),
            |(worker, scratch), (src, dst)| convert_band(worker, src, channels, dst, scratch),
        );
    out
}

impl OutputTransform {
    /// The output space this transform encodes to.
    pub fn space(&self) -> OutputSpace {
        self.space
    }

    /// The rendering intent this transform was built with.
    pub fn intent(&self) -> Intent {
        self.intent
    }

    /// Encodes a linear Rec.2020 image as interleaved 16-bit RGB in the
    /// output space, row-major, `width * height * 3` values. This is the
    /// one place the pipeline quantises to 16 bits (ground rule 4). Values
    /// outside `[0, 1]` saturate at the quantisation. Runs in parallel; see
    /// the module documentation.
    ///
    /// # Panics
    ///
    /// If Little-CMS cannot build, for another worker, a transform it
    /// already built once, which only happens on allocation failure.
    pub fn apply(&self, image: &Image) -> Vec<u16> {
        let (space, intent) = (self.space, self.intent);
        let build = move || {
            build_output(space, intent, PixelFormat::RGB_16, "16-bit")
                .expect("rebuilding a transform that was built before")
        };
        convert(&self.to_u16, build, image).into_flattened()
    }

    /// As [`OutputTransform::apply`] but 8 bits per channel, for JPEG and
    /// display.
    ///
    /// # Panics
    ///
    /// As [`OutputTransform::apply`].
    pub fn apply_u8(&self, image: &Image) -> Vec<u8> {
        let (space, intent) = (self.space, self.intent);
        let build = move || {
            build_output(space, intent, PixelFormat::RGB_8, "8-bit")
                .expect("rebuilding a transform that was built before")
        };
        convert(&self.to_u8, build, image).into_flattened()
    }
}

impl Clone for OutputTransform {
    /// Rebuilds the Little-CMS transform for the same space and intent, so
    /// each thread can own one.
    ///
    /// # Panics
    ///
    /// If Little-CMS cannot rebuild a transform it already built once,
    /// which only happens on allocation failure.
    fn clone(&self) -> Self {
        working_to_output(self.space, self.intent)
            .expect("rebuilding a transform that was built before")
    }
}

/// Floats per parallel band of [`icc_linearise_native`].
const LINEARISE_BAND_FLOATS: usize = 1 << 18;

/// Opens an embedded ICC profile and checks that it is an RGB matrix/TRC
/// profile with no LUT-based transform.
fn open_matrix_trc(icc: &[u8]) -> Result<Profile, CmsError> {
    let profile =
        Profile::new_icc(icc).map_err(|e| lcms_error("opening the embedded ICC profile", e))?;
    let space = profile.color_space();
    if space != ColorSpaceSignature::RgbData {
        return Err(CmsError::Unsupported(format!(
            "embedded ICC profile is {space:?}, only RGB profiles can be linearised"
        )));
    }
    let lut = [
        TagSignature::AToB0Tag,
        TagSignature::AToB1Tag,
        TagSignature::AToB2Tag,
    ]
    .into_iter()
    .any(|sig| profile.has_tag(sig));
    if lut || !profile.is_matrix_shaper() {
        return Err(CmsError::Unsupported(
            "embedded ICC profile is LUT-based; only matrix/TRC profiles are supported".into(),
        ));
    }
    Ok(profile)
}

/// The red, green and blue TRCs of a matrix/TRC profile, copied out of it.
fn trc_curves(profile: &Profile) -> Result<[ToneCurve; 3], CmsError> {
    let read = |sig| match profile.read_tag(sig) {
        Tag::ToneCurve(curve) => Ok(curve.to_owned()),
        _ => Err(CmsError::Unsupported(format!(
            "embedded ICC profile has no usable {sig:?}"
        ))),
    };
    Ok([
        read(TagSignature::RedTRCTag)?,
        read(TagSignature::GreenTRCTag)?,
        read(TagSignature::BlueTRCTag)?,
    ])
}

/// One colorant tag of a matrix/TRC profile, CIE XYZ relative to D50.
fn colorant(profile: &Profile, sig: TagSignature) -> Result<[f64; 3], CmsError> {
    match profile.read_tag(sig) {
        Tag::CIEXYZ(xyz) => Ok([xyz.X, xyz.Y, xyz.Z]),
        _ => Err(CmsError::Unsupported(format!(
            "embedded ICC profile has no usable {sig:?}"
        ))),
    }
}

/// The step 1 path for a TIFF with an embedded matrix/TRC ICC profile
/// (`docs/MATHS.md` step 1, "honour the embedded ICC if matrix/TRC,
/// linearise with its TRC"; ADR 0012, Ruling 29).
///
/// Each channel of `encoded` (values nominally in `[0, 1]`; negatives are
/// read as 0) is put through the profile's own TRC for that channel,
/// Little-CMS evaluating the curve in floating point, so the image comes
/// back linear **in the profile's own primaries**, not in Rec.2020: the
/// density and inversion of steps 3 to 6 see the scan's channels as they
/// were captured. The second value is the step 7 matrix from those
/// primaries to linear Rec.2020,
/// `XYZ_TO_REC2020 * bradford(D50 -> D65) * C` with `C` the colorant
/// matrix ([`crate::matrix::icc_colorants_to_rec2020`]), which maps
/// `(1, 1, 1)` to Rec.2020 `(1, 1, 1)`.
///
/// Runs in parallel over bands, each rayon worker reading its own copy of
/// the curves (Little-CMS curves are not `Sync`).
///
/// # Errors
///
/// - [`CmsError::Lcms`] when the bytes are not a profile Little-CMS can
///   open.
/// - [`CmsError::Unsupported`] when the profile is not RGB, is LUT-based
///   (has an `AToB` tag or lacks the colorant and TRC tags), or a colorant
///   or TRC tag cannot be read.
///
/// # Panics
///
/// If Little-CMS cannot reopen, for another worker, a profile it already
/// opened, which only happens on allocation failure.
pub fn icc_linearise_native(icc: &[u8], encoded: &Image) -> Result<(Image, Matrix3), CmsError> {
    let profile = open_matrix_trc(icc)?;
    let matrix = icc_colorants_to_rec2020(
        colorant(&profile, TagSignature::RedColorantTag)?,
        colorant(&profile, TagSignature::GreenColorantTag)?,
        colorant(&profile, TagSignature::BlueColorantTag)?,
    );
    let curves = trc_curves(&profile)?;
    let channels = encoded.channels();
    let band = (LINEARISE_BAND_FLOATS / channels).max(1) * channels;
    let mut linear = encoded.clone();
    let apply = |curves: &[ToneCurve; 3], band: &mut [f32]| {
        for px in band.chunks_exact_mut(channels) {
            for (v, curve) in px.iter_mut().zip(curves) {
                *v = curve.eval(v.max(0.0));
            }
        }
    };
    if linear.data().len() <= band {
        apply(&curves, linear.data_mut());
    } else {
        linear.data_mut().par_chunks_mut(band).for_each_init(
            || {
                let profile = open_matrix_trc(icc).expect("reopening a profile opened before");
                trc_curves(&profile).expect("reading curves read before")
            },
            |curves, band| apply(curves, band),
        );
    }
    Ok((linear, matrix))
}

/// The step 1 fallback for 16-bit integer TIFFs with no ICC profile
/// (R-decode): assume a gamma 2.2 encoding and undo it per channel with
/// `v^2.2`. Negative inputs are clipped to 0 first (a fractional power of a
/// negative number is undefined); values above 1 pass through the same
/// power.
///
/// This is a per-pixel operation living outside `negg-render`, which
/// ground rule 3 of the plan would normally forbid; it is kept here because
/// it is a transfer function, the same kind of thing the ICC path above
/// does, and `negg-render` must stay free of colour management.
pub fn gamma22_to_linear(encoded: &Image) -> Image {
    let data = encoded
        .data()
        .iter()
        .map(|&v| v.max(0.0).powf(2.2))
        .collect();
    Image::from_vec(encoded.width(), encoded.height(), data).expect("the pixel count is unchanged")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lab::srgb_oetf;
    use crate::profiles::profile_bytes;
    use lcms2::ToneCurve;

    const ALL: [OutputSpace; 5] = [
        OutputSpace::Srgb,
        OutputSpace::AdobeRgb,
        OutputSpace::DisplayP3,
        OutputSpace::ProPhoto,
        OutputSpace::Rec2020,
    ];

    fn one_pixel(rgb: [f32; 3]) -> Image {
        Image::from_vec(1, 1, rgb.to_vec()).unwrap()
    }

    fn assert_within(got: &[u16], want: u16, tol: u16, what: &str) {
        for (i, &g) in got.iter().enumerate() {
            assert!(
                g.abs_diff(want) <= tol,
                "{what}: channel {i} got {g}, want {want} within {tol}"
            );
        }
    }

    #[test]
    fn srgb_white_is_65535() {
        let t = working_to_output(OutputSpace::Srgb, Intent::default()).unwrap();
        let out = t.apply(&one_pixel([1.0, 1.0, 1.0]));
        assert_eq!(out.len(), 3);
        assert_within(&out, 65535, 1, "sRGB white");
        let out8 = t.apply_u8(&one_pixel([1.0, 1.0, 1.0]));
        assert_eq!(out8, [255, 255, 255]);
    }

    #[test]
    fn black_is_zero() {
        let t = working_to_output(OutputSpace::Srgb, Intent::default()).unwrap();
        assert_eq!(t.apply(&one_pixel([0.0, 0.0, 0.0])), [0, 0, 0]);
        assert_eq!(t.apply_u8(&one_pixel([0.0, 0.0, 0.0])), [0, 0, 0]);
    }

    #[test]
    fn rec2020_mid_grey_to_srgb_is_about_0_461() {
        let t = working_to_output(OutputSpace::Srgb, Intent::default()).unwrap();
        let out = t.apply(&one_pixel([0.18, 0.18, 0.18]));
        // srgb_oetf(0.18) = 0.4613, times 65535 = 30231; 1% is about 302.
        let want = (srgb_oetf(0.18) * 65535.0).round() as u16;
        assert_within(&out, want, 302, "sRGB mid grey");
        // Grey stays neutral: both spaces share the D65 white.
        assert!(
            out[0].abs_diff(out[1]) <= 1 && out[1].abs_diff(out[2]) <= 1,
            "{out:?}"
        );
        let out8 = t.apply_u8(&one_pixel([0.18, 0.18, 0.18]));
        assert!((117..=118).contains(&out8[1]), "{out8:?}");
    }

    #[test]
    fn display_p3_and_srgb_map_white_to_white_within_one_code_value() {
        for space in [OutputSpace::Srgb, OutputSpace::DisplayP3] {
            for intent in [Intent::Perceptual, Intent::RelativeColorimetric] {
                let t = working_to_output(space, intent).unwrap();
                let out = t.apply(&one_pixel([1.0, 1.0, 1.0]));
                assert_within(&out, 65535, 1, &format!("{space:?} {intent:?} white"));
                let out8 = t.apply_u8(&one_pixel([1.0, 1.0, 1.0]));
                assert!(
                    out8.iter().all(|&v| v >= 254),
                    "{space:?} {intent:?}: {out8:?}"
                );
            }
        }
    }

    #[test]
    fn every_space_maps_white_to_white_and_black_to_black() {
        for space in ALL {
            let t = working_to_output(space, Intent::RelativeColorimetric).unwrap();
            assert_eq!(t.space(), space);
            assert_eq!(t.intent(), Intent::RelativeColorimetric);
            let out = t.apply(&one_pixel([1.0, 1.0, 1.0]));
            assert_within(&out, 65535, 1, &format!("{space:?} white"));
            let out = t.apply(&one_pixel([0.0, 0.0, 0.0]));
            assert_within(&out, 0, 1, &format!("{space:?} black"));
        }
    }

    #[test]
    fn out_of_range_values_saturate() {
        let t = working_to_output(OutputSpace::Srgb, Intent::default()).unwrap();
        let out = t.apply(&one_pixel([2.0, -0.5, 1.0]));
        assert_eq!(out, [65535, 0, 65535]);
    }

    #[test]
    fn rec2020_export_is_the_bt709_curve() {
        // Rec.2020 output has the working primaries, so the transform is the
        // OETF alone: 0.18 -> 1.099 * 0.18^0.45 - 0.099 = 0.4094.
        let t = working_to_output(OutputSpace::Rec2020, Intent::default()).unwrap();
        let out = t.apply(&one_pixel([0.18, 0.18, 0.18]));
        let want = ((1.099 * 0.18f64.powf(0.45) - 0.099) * 65535.0).round() as u16;
        assert_within(&out, want, 2, "Rec.2020 mid grey");
        // Below the knee the curve is linear: 0.01 -> 4.5 * 0.01 = 0.045.
        let out = t.apply(&one_pixel([0.01, 0.01, 0.01]));
        let want = (0.045 * 65535.0f64).round() as u16;
        assert_within(&out, want, 2, "Rec.2020 toe");
    }

    #[test]
    fn images_longer_than_a_chunk_are_converted_whole() {
        let pixels = CHUNK_PIXELS * 2 + 7;
        let mut data = Vec::with_capacity(pixels * 3);
        for i in 0..pixels {
            let v = (i % 97) as f32 / 96.0;
            data.extend_from_slice(&[v, 0.5, 1.0 - v]);
        }
        let image = Image::from_vec(pixels, 1, data.clone()).unwrap();
        let t = working_to_output(OutputSpace::Srgb, Intent::default()).unwrap();
        let out = t.apply(&image);
        assert_eq!(out.len(), pixels * 3);
        let out8 = t.apply_u8(&image);
        assert_eq!(out8.len(), pixels * 3);
        // Each pixel equals the same pixel converted on its own.
        for i in [
            0,
            1,
            CHUNK_PIXELS - 1,
            CHUNK_PIXELS,
            CHUNK_PIXELS + 1,
            pixels - 1,
        ] {
            let single = t.apply(&one_pixel([data[i * 3], data[i * 3 + 1], data[i * 3 + 2]]));
            assert_eq!(&out[i * 3..i * 3 + 3], &single[..], "pixel {i}");
        }
    }

    #[test]
    fn parallel_bands_equal_one_sequential_transform() {
        // Three and a bit bands, so several workers each build their own
        // transform; every pixel must equal the single-transform result.
        let (w, h) = (1000, BAND_PIXELS * 3 / 1000 + 2);
        let mut data = Vec::with_capacity(w * h * 3);
        for i in 0..w * h {
            let v = (i % 1009) as f32 / 1008.0;
            let u = (i % 211) as f32 / 210.0;
            data.extend_from_slice(&[v, u * 1.1 - 0.05, 1.0 - v * u]);
        }
        let image = Image::from_vec(w, h, data).unwrap();
        let t = working_to_output(OutputSpace::AdobeRgb, Intent::Perceptual).unwrap();
        let seq16: Vec<u16> = {
            let src: Vec<[f32; 3]> = image.data().as_chunks::<3>().0.to_vec();
            let mut dst = vec![[0u16; 3]; src.len()];
            t.to_u16.transform_pixels(&src, &mut dst);
            dst.into_flattened()
        };
        let seq8: Vec<u8> = {
            let src: Vec<[f32; 3]> = image.data().as_chunks::<3>().0.to_vec();
            let mut dst = vec![[0u8; 3]; src.len()];
            t.to_u8.transform_pixels(&src, &mut dst);
            dst.into_flattened()
        };
        assert!(image.width() * image.height() > BAND_PIXELS * 3);
        assert_eq!(t.apply(&image), seq16);
        assert_eq!(t.apply_u8(&image), seq8);
    }

    #[test]
    fn empty_image_gives_empty_output() {
        let t = working_to_output(OutputSpace::Srgb, Intent::default()).unwrap();
        assert!(t.apply(&Image::new(0, 4)).is_empty());
        assert!(t.apply_u8(&Image::new(3, 0)).is_empty());
    }

    #[test]
    fn clone_gives_the_same_result() {
        let t = working_to_output(OutputSpace::DisplayP3, Intent::RelativeColorimetric).unwrap();
        let c = t.clone();
        assert_eq!(c.space(), t.space());
        assert_eq!(c.intent(), t.intent());
        let image = one_pixel([0.3, 0.6, 0.1]);
        assert_eq!(c.apply(&image), t.apply(&image));
        assert_eq!(c.apply_u8(&image), t.apply_u8(&image));
    }

    #[test]
    fn output_transform_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<OutputTransform>();
    }

    #[test]
    fn gamma22_of_half_is_half_to_the_2_2() {
        let out = gamma22_to_linear(&one_pixel([0.5, 1.0, 0.0]));
        let want = 0.5f32.powf(2.2);
        assert!(
            (out.pixel(0, 0)[0] - want).abs() <= 1e-6,
            "{:?}",
            out.pixel(0, 0)
        );
        assert!((out.pixel(0, 0)[1] - 1.0).abs() <= 1e-6);
        assert_eq!(out.pixel(0, 0)[2], 0.0);
        assert_eq!((out.width(), out.height()), (1, 1));
    }

    #[test]
    fn gamma22_clips_negatives_and_passes_above_one() {
        let out = gamma22_to_linear(&one_pixel([-0.25, 2.0, 0.25]));
        let p = out.pixel(0, 0);
        assert_eq!(p[0], 0.0);
        assert!((p[1] - 2.0f32.powf(2.2)).abs() <= 1e-5);
        assert!((p[2] - 0.25f32.powf(2.2)).abs() <= 1e-6);
    }

    fn rgb_of(m: Matrix3, v: [f32; 3]) -> [f32; 3] {
        crate::matrix::mul_vec(m, v)
    }

    #[test]
    fn native_srgb_grey_gives_linear_grey_and_a_white_normalised_matrix() {
        let icc = profile_bytes(OutputSpace::Srgb).unwrap();
        let v = srgb_oetf(0.18);
        let (out, m) = icc_linearise_native(&icc, &one_pixel([v, v, v])).unwrap();
        let p = out.pixel(0, 0);
        for c in p {
            assert!((c - 0.18).abs() <= 1e-3, "{p:?}");
        }
        // The step 7 matrix maps the profile's white to Rec.2020 white.
        for c in rgb_of(m, [1.0, 1.0, 1.0]) {
            assert!((c - 1.0).abs() <= 1e-3, "{m:?}");
        }
        // And it is the BT.2087 sRGB to Rec.2020 matrix.
        let want = [
            [0.6274, 0.3293, 0.0433],
            [0.0691, 0.9195, 0.0114],
            [0.0164, 0.0880, 0.8956],
        ];
        for i in 0..3 {
            for j in 0..3 {
                assert!((m[i][j] - want[i][j]).abs() <= 2e-3, "{m:?}");
            }
        }
    }

    #[test]
    fn native_linearisation_stays_in_the_profile_primaries() {
        // sRGB red is linearised to (1, 0, 0) in sRGB's own primaries; only
        // the step 7 matrix takes it to Rec.2020.
        let icc = profile_bytes(OutputSpace::Srgb).unwrap();
        let (out, m) = icc_linearise_native(&icc, &one_pixel([1.0, 0.0, 0.0])).unwrap();
        let p = out.pixel(0, 0);
        assert!((p[0] - 1.0).abs() <= 1e-4 && p[1].abs() <= 1e-6 && p[2].abs() <= 1e-6);
        let red = rgb_of(m, p);
        let want = [0.6274, 0.0691, 0.0164];
        for i in 0..3 {
            assert!((red[i] - want[i]).abs() <= 2e-3, "{red:?} vs {want:?}");
        }
        // Negative inputs are read as 0.
        let (out, _) = icc_linearise_native(&icc, &one_pixel([-0.5, 0.0, 0.0])).unwrap();
        assert_eq!(out.pixel(0, 0)[0], 0.0);
    }

    #[test]
    fn native_prophoto_white_maps_to_rec2020_white() {
        let icc = profile_bytes(OutputSpace::ProPhoto).unwrap();
        let (out, m) = icc_linearise_native(&icc, &one_pixel([1.0, 1.0, 1.0])).unwrap();
        for c in rgb_of(m, out.pixel(0, 0)) {
            assert!((c - 1.0).abs() <= 1e-3, "{m:?}");
        }
        // ProPhoto green is outside Rec.2020: some channel goes negative
        // rather than being clipped.
        let (out, m) = icc_linearise_native(&icc, &one_pixel([0.0, 1.0, 0.0])).unwrap();
        assert!(rgb_of(m, out.pixel(0, 0)).iter().any(|&c| c < 0.0));
    }

    #[test]
    fn export_then_native_linearise_round_trips_the_neutral_axis() {
        let mut data = Vec::new();
        for i in 0..=20 {
            let v = i as f32 / 20.0;
            data.extend_from_slice(&[v, v, v]);
        }
        let pixels = data.len() / 3;
        let image = Image::from_vec(pixels, 1, data.clone()).unwrap();
        let t = working_to_output(OutputSpace::Srgb, Intent::RelativeColorimetric).unwrap();
        let encoded: Vec<f32> = t
            .apply(&image)
            .into_iter()
            .map(|v| f32::from(v) / 65535.0)
            .collect();
        let encoded = Image::from_vec(pixels, 1, encoded).unwrap();
        let (back, m) =
            icc_linearise_native(&profile_bytes(OutputSpace::Srgb).unwrap(), &encoded).unwrap();
        for (i, px) in back.data().as_chunks::<3>().0.iter().enumerate() {
            let rec2020 = rgb_of(m, *px);
            for c in rec2020 {
                assert!((c - data[i * 3]).abs() <= 2e-3, "pixel {i}: {rec2020:?}");
            }
        }
    }

    #[test]
    fn native_linearisation_of_a_large_image_matches_pixel_by_pixel() {
        // More than one parallel band: every pixel equals the one-pixel
        // result.
        let icc = profile_bytes(OutputSpace::AdobeRgb).unwrap();
        let (w, h) = (1000, LINEARISE_BAND_FLOATS / 3000 * 3 + 5);
        let data: Vec<f32> = (0..w * h * 3).map(|i| (i % 4099) as f32 / 4098.0).collect();
        let image = Image::from_vec(w, h, data.clone()).unwrap();
        let (out, _) = icc_linearise_native(&icc, &image).unwrap();
        for i in [0, 1, w * h / 2, w * h - 1] {
            let one = one_pixel([data[i * 3], data[i * 3 + 1], data[i * 3 + 2]]);
            let (single, _) = icc_linearise_native(&icc, &one).unwrap();
            assert_eq!(&out.data()[i * 3..i * 3 + 3], single.data(), "pixel {i}");
        }
    }

    #[test]
    fn native_rejects_non_rgb_lut_based_and_garbage_profiles() {
        let grey = Profile::new_gray(
            &lcms2::CIExyY {
                x: 0.3127,
                y: 0.3290,
                Y: 1.0,
            },
            &ToneCurve::new(2.2),
        )
        .unwrap()
        .icc()
        .unwrap();
        let err = icc_linearise_native(&grey, &one_pixel([0.5, 0.5, 0.5])).unwrap_err();
        assert!(matches!(err, CmsError::Unsupported(_)), "{err:?}");
        assert!(err.to_string().contains("GrayData"), "{err}");

        // A device link built from an sRGB transform is RGB but carries an
        // AToB0 pipeline and no colorants: LUT-based.
        let srgb = Profile::new_srgb();
        let link: Transform<[u8; 3], [u8; 3]> = Transform::new(
            &srgb,
            PixelFormat::RGB_8,
            &srgb,
            PixelFormat::RGB_8,
            lcms2::Intent::Perceptual,
        )
        .unwrap();
        let lut = Profile::new_device_link(&link, 4.3, lcms2::Flags::default())
            .unwrap()
            .icc()
            .unwrap();
        let err = icc_linearise_native(&lut, &one_pixel([0.5; 3])).unwrap_err();
        assert!(matches!(err, CmsError::Unsupported(_)), "{err:?}");
        assert!(err.to_string().contains("LUT-based"), "{err}");

        let err =
            icc_linearise_native(b"this is not an ICC profile", &one_pixel([0.5; 3])).unwrap_err();
        assert!(matches!(err, CmsError::Lcms(_)), "{err:?}");
    }
}
