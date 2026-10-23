// SPDX-License-Identifier: MPL-2.0

//! RAW decoding through rawler.
//!
//! rawler reads the file, finds the camera in its catalogue and hands back
//! the sensor data with its black and white levels, CFA pattern, active
//! area and colour matrices. This module then does exactly three things:
//! scales each sample so the calibrated black level maps to 0 and the
//! white level to 1 (see [`crate::levels`]), crops to the active area, and
//! bilinearly demosaics 2x2 Bayer mosaics (see [`crate::demosaic`]). It
//! applies no white balance, no colour matrix and no transfer curve, so
//! the result is camera-native linear RGB at unity gain.
//!
//! The colour matrix rawler reports (XYZ to camera, D65 when the file has
//! one) is passed up in [`Decoded::xyz_to_camera`], with the XYZ white of
//! the illuminant it was measured under in [`Decoded::matrix_white`], for
//! step 7 to use; it is not applied here. Orientation metadata is likewise reported by rawler but
//! not applied: the image comes back in sensor orientation.

use std::path::Path;

use negg_types::Image;
use negg_types::image::CHANNELS;
use rawler::RawImage;
use rawler::imgop::Rect;
use rawler::imgop::xyz::Illuminant;
use rawler::rawimage::RawPhotometricInterpretation;

use crate::demosaic::{BLUE, BayerPattern, GREEN, RED, bilinear};
use crate::error::DecodeError;
use crate::levels::{LevelTable, check_tables, scale_sample};
use crate::{Decoded, Transfer};

/// Decodes a camera RAW file into linear camera RGB.
///
/// See the module documentation for what is and is not applied. Bayer
/// sensors with a 2x2 RGB pattern and linear (already RGB) DNGs are
/// supported; monochrome sensors, X-Trans and other CFA layouts return
/// [`DecodeError::Unsupported`].
///
/// # Errors
///
/// [`DecodeError::Io`] if the file cannot be opened, [`DecodeError::Raw`]
/// if rawler cannot decode it, [`DecodeError::Unsupported`] for cameras
/// or layouts outside the phase 1 scope, and [`DecodeError::Dimensions`]
/// if rawler's buffer does not match the size it declares.
pub fn decode_raw(path: &Path) -> Result<Decoded, DecodeError> {
    // rawler folds a missing file into its generic failure string; open it
    // first so the caller gets a proper I/O error instead.
    drop(std::fs::File::open(path)?);
    let raw = rawler::decode_file(path)?;
    from_raw_image(&raw)
}

/// Converts a decoded rawler image into [`Decoded`].
pub(crate) fn from_raw_image(raw: &RawImage) -> Result<Decoded, DecodeError> {
    let mut warnings = Vec::new();
    let camera = camera_name(raw);
    let matrix = colour_matrix(raw, &mut warnings);
    let xyz_to_camera = matrix.map(|(m, _)| m);
    let matrix_white = matrix.map(|(_, white)| white);
    let (black, white) = level_tables(raw)?;
    check_tables(&black, &white)?;
    // With no catalogue black level and masked sensor areas, rawler takes
    // the mean of the masked pixels of this frame as the black level.
    if raw.camera.blacklevel.is_none() && !raw.blackareas.is_empty() {
        warnings.push("black level estimated from masked sensor area for this frame".into());
    }

    let data = raw.data.as_f32();
    let samples = raw
        .width
        .checked_mul(raw.height)
        .and_then(|n| n.checked_mul(raw.cpp));
    if samples != Some(data.len()) {
        return Err(DecodeError::Dimensions(negg_types::Error::Dimensions {
            width: raw.width,
            height: raw.height,
            len: data.len(),
        }));
    }
    let crop = active_area(raw);

    let (image, black_level, white_level) = match &raw.photometric {
        RawPhotometricInterpretation::Cfa(cfg) => {
            if raw.cpp != 1 {
                return Err(DecodeError::Unsupported(format!(
                    "CFA image with {} samples per pixel",
                    raw.cpp
                )));
            }
            let pattern = bayer_pattern(&cfg.cfa)?;
            let image = decode_bayer(&data, raw.width, pattern, &black, &white, crop)?;
            let level_of = |table: &LevelTable| {
                let mut out = [0.0f32; CHANNELS];
                for (channel, value) in out.iter_mut().enumerate() {
                    let (x, y) = pattern
                        .first_site(channel as u8)
                        .expect("validated Bayer pattern has every colour");
                    *value = table.at(y, x, 0);
                }
                out
            };
            (image, level_of(&black), level_of(&white))
        }
        RawPhotometricInterpretation::LinearRaw => {
            if raw.cpp != CHANNELS {
                return Err(DecodeError::Unsupported(format!(
                    "linear RAW with {} samples per pixel; only RGB is read",
                    raw.cpp
                )));
            }
            let image = decode_linear(&data, raw.width, &black, &white, crop)?;
            let level_of = |table: &LevelTable| {
                let mut out = [0.0f32; CHANNELS];
                for (channel, value) in out.iter_mut().enumerate() {
                    *value = table.at(0, 0, channel);
                }
                out
            };
            (image, level_of(&black), level_of(&white))
        }
        RawPhotometricInterpretation::BlackIsZero => {
            return Err(DecodeError::Unsupported(
                "monochrome sensor; only colour sensors are read".into(),
            ));
        }
    };

    Ok(Decoded {
        image,
        transfer: Transfer::Linear,
        xyz_to_camera,
        matrix_white,
        camera,
        white_level,
        black_level,
        warnings,
    })
}

/// Scales, crops and demosaics a Bayer mosaic.
fn decode_bayer(
    data: &[f32],
    width: usize,
    pattern: BayerPattern,
    black: &LevelTable,
    white: &LevelTable,
    crop: Rect,
) -> Result<Image, DecodeError> {
    let (x0, y0, w, h) = (crop.p.x, crop.p.y, crop.d.w, crop.d.h);
    let mut mosaic = Vec::with_capacity(w * h);
    for row in y0..y0 + h {
        let line = &data[row * width..row * width + width];
        for (col, &v) in line.iter().enumerate().take(x0 + w).skip(x0) {
            mosaic.push(scale_sample(
                v,
                black.at(row, col, 0),
                white.at(row, col, 0),
            ));
        }
    }
    bilinear(w, h, &mosaic, pattern.shifted(x0, y0))
}

/// Scales and crops an image that is already RGB (a linear DNG).
fn decode_linear(
    data: &[f32],
    width: usize,
    black: &LevelTable,
    white: &LevelTable,
    crop: Rect,
) -> Result<Image, DecodeError> {
    let (x0, y0, w, h) = (crop.p.x, crop.p.y, crop.d.w, crop.d.h);
    let mut out = Vec::with_capacity(w * h * CHANNELS);
    for row in y0..y0 + h {
        for col in x0..x0 + w {
            let i = (row * width + col) * CHANNELS;
            for plane in 0..CHANNELS {
                out.push(scale_sample(
                    data[i + plane],
                    black.at(row, col, plane),
                    white.at(row, col, plane),
                ));
            }
        }
    }
    Ok(Image::from_vec(w, h, out)?)
}

/// The active area clamped to the image, or the whole image when rawler
/// reports none.
fn active_area(raw: &RawImage) -> Rect {
    let full = Rect::new(
        rawler::imgop::Point::zero(),
        rawler::imgop::Dim2::new(raw.width, raw.height),
    );
    match raw.active_area {
        Some(area) if !area.is_empty() => {
            let x0 = area.p.x.min(raw.width);
            let y0 = area.p.y.min(raw.height);
            let w = area.d.w.min(raw.width - x0);
            let h = area.d.h.min(raw.height - y0);
            if w == 0 || h == 0 {
                full
            } else {
                Rect::new(
                    rawler::imgop::Point::new(x0, y0),
                    rawler::imgop::Dim2::new(w, h),
                )
            }
        }
        _ => full,
    }
}

/// Converts rawler's CFA into a [`BayerPattern`].
fn bayer_pattern(cfa: &rawler::CFA) -> Result<BayerPattern, DecodeError> {
    if cfa.width != 2 || cfa.height != 2 {
        return Err(DecodeError::Unsupported(format!(
            "{}x{} CFA '{}'; only 2x2 Bayer patterns are demosaiced in phase 1",
            cfa.width, cfa.height, cfa.name
        )));
    }
    let mut sites = [0u8; 4];
    for (i, site) in sites.iter_mut().enumerate() {
        *site = match cfa.color_at(i / 2, i % 2) {
            0 => RED,
            1 => GREEN,
            2 => BLUE,
            other => {
                return Err(DecodeError::Unsupported(format!(
                    "CFA '{}' has a non-RGB filter (colour index {other})",
                    cfa.name
                )));
            }
        };
    }
    BayerPattern::new(sites)
}

/// Builds the black and white level tables from rawler's levels.
///
/// rawler indexes a four-entry level list by CFA position (row parity,
/// then column parity), which is the DNG `BlackLevelRepeatDim` order, and
/// its black level already carries its repeat dimensions. A white level
/// list has one entry, one per plane, or one per 2x2 site.
///
/// Known limitation, inherited from rawler and not corrected here: a DNG
/// with a 2x2 `BlackLevelRepeatDim`, distinct per-site values and an odd
/// `ActiveArea` origin has its black-level table indexed from the sensor
/// origin rather than the active-area origin, misaligning the sites by a
/// fraction of a code value.
fn level_tables(raw: &RawImage) -> Result<(LevelTable, LevelTable), DecodeError> {
    let bl = &raw.blacklevel;
    let black = LevelTable::new(bl.as_vec(), bl.width, bl.height, bl.cpp)?;
    let wl: Vec<f32> = raw.whitelevel.as_vec();
    let white = match wl.len() {
        1 => LevelTable::uniform(wl[0]),
        n if n == raw.cpp => LevelTable::new(wl, 1, 1, n)?,
        4 if raw.cpp == 1 => LevelTable::new(wl, 2, 2, 1)?,
        n => {
            return Err(DecodeError::Unsupported(format!(
                "white level list has {n} entries for {} samples per pixel",
                raw.cpp
            )));
        }
    };
    Ok((black, white))
}

/// The camera's cleaned make and model, or `None` when rawler has neither.
fn camera_name(raw: &RawImage) -> Option<String> {
    let name = format!("{} {}", raw.clean_make.trim(), raw.clean_model.trim());
    let name = name.trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// CIE XYZ (`Y = 1`) of the D65 illuminant, the white assumed for a matrix
/// whose illuminant has no tabulated white here.
pub const D65_WHITE: [f32; 3] = [0.95047, 1.0, 1.08883];

/// The CIE XYZ white (`Y = 1`, 2-degree observer) of the illuminant a
/// colour matrix was measured under: D65, D50, D55, D75 and A from the CIE
/// tables, `None` for any other illuminant (the caller assumes D65 and
/// warns).
pub fn illuminant_white(illu: Illuminant) -> Option<[f32; 3]> {
    match illu {
        Illuminant::D65 => Some(D65_WHITE),
        Illuminant::D50 => Some([0.96422, 1.0, 0.82521]),
        Illuminant::D55 => Some([0.95682, 1.0, 0.92149]),
        Illuminant::D75 => Some([0.94972, 1.0, 1.22638]),
        Illuminant::A => Some([1.09850, 1.0, 0.35585]),
        _ => None,
    }
}

/// Picks the XYZ-to-camera matrix and the XYZ white of its illuminant:
/// the D65 one when present, else the lowest-numbered illuminant rawler
/// has (with a warning), else the catalogue matrix with no recorded
/// illuminant (with a warning; D65 assumed), else `None` (with a
/// warning). An illuminant with no tabulated white in
/// [`illuminant_white`] is taken as D65, with a warning.
fn colour_matrix(raw: &RawImage, warnings: &mut Vec<String>) -> Option<([[f32; 3]; 3], [f32; 3])> {
    if let Some(m) = raw.color_matrix.get(&Illuminant::D65) {
        return flat_to_3x3(m, "D65", warnings).map(|m| (m, D65_WHITE));
    }
    let mut illuminants: Vec<Illuminant> = raw.color_matrix.keys().copied().collect();
    illuminants.sort_by_key(|&i| (illuminant_rank(i), i));
    if let Some(&illu) = illuminants.first() {
        warnings.push(format!("no D65 colour matrix; using the {illu:?} matrix"));
        let white = illuminant_white(illu).unwrap_or_else(|| {
            warnings.push(format!(
                "no tabulated white for the {illu:?} illuminant; assuming D65 for the matrix"
            ));
            D65_WHITE
        });
        return flat_to_3x3(&raw.color_matrix[&illu], &format!("{illu:?}"), warnings)
            .map(|m| (m, white));
    }
    let catalogue = raw.xyz_to_cam;
    if catalogue.iter().take(3).flatten().any(|&v| v != 0.0) {
        warnings.push(
            "colour matrix illuminant not recorded; using the camera catalogue matrix and \
             assuming D65"
                .into(),
        );
        return Some(([catalogue[0], catalogue[1], catalogue[2]], D65_WHITE));
    }
    warnings.push("no colour matrix available for this camera".into());
    None
}

/// Preference order for a fallback illuminant when there is no D65
/// matrix: the daylight family nearest D65 first, then the rest.
fn illuminant_rank(illu: Illuminant) -> u8 {
    match illu {
        Illuminant::D65 => 0,
        Illuminant::D55 => 1,
        Illuminant::D75 => 2,
        Illuminant::D50 => 3,
        Illuminant::Daylight => 4,
        Illuminant::FineWeather => 5,
        Illuminant::CloudyWeather => 6,
        Illuminant::Shade => 7,
        Illuminant::Flash => 8,
        Illuminant::DaylightFluorescent => 9,
        Illuminant::DaylightWhiteFluorescent => 10,
        _ => 20,
    }
}

/// Reshapes a nine-entry row-major matrix; warns and returns `None` for
/// any other length (four-colour sensors carry twelve).
fn flat_to_3x3(flat: &[f32], label: &str, warnings: &mut Vec<String>) -> Option<[[f32; 3]; 3]> {
    if flat.len() != 9 {
        warnings.push(format!(
            "{label} colour matrix has {} entries; expected 9, ignoring it",
            flat.len()
        ));
        return None;
    }
    Some([
        [flat[0], flat[1], flat[2]],
        [flat[3], flat[4], flat[5]],
        [flat[6], flat[7], flat[8]],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{DngSpec, TempDir, mosaic_from_scene};

    const BLACK: u16 = 512;
    const WHITE: u16 = 16383;

    /// The scene every synthetic DNG test samples: a flat patch on the
    /// left half and a gradient on the right.
    fn scene(x: usize, _y: usize) -> [u16; 3] {
        if x < 8 {
            [4096, 8192, 12288]
        } else {
            let t = (x - 8) as u32 * 500;
            [(1000 + t) as u16, (2000 + t) as u16, (3000 + t) as u16]
        }
    }

    fn spec(data: Vec<u16>) -> DngSpec {
        DngSpec {
            width: 16,
            height: 16,
            black: BLACK,
            white: WHITE,
            pattern: "RGGB",
            make: "Negg",
            model: "Synthetic",
            matrix: Some([[0.7, 0.1, 0.05], [-0.2, 1.1, 0.1], [0.0, 0.1, 0.6]]),
            active_area: None,
            data,
        }
    }

    fn expected(value: u16) -> f32 {
        scale_sample(f32::from(value), f32::from(BLACK), f32::from(WHITE))
    }

    #[test]
    fn synthetic_dng_decodes_to_scaled_camera_rgb() {
        let dir = TempDir::new("dng");
        let path = dir.path("flat.dng");
        let data = mosaic_from_scene(16, 16, "RGGB", scene);
        spec(data).write(&path);

        let d = decode_raw(&path).unwrap();
        assert_eq!((d.image.width(), d.image.height()), (16, 16));
        assert_eq!(d.transfer, Transfer::Linear);
        assert_eq!(d.camera.as_deref(), Some("Negg Synthetic"));
        assert_eq!(d.black_level, [512.0; 3]);
        assert_eq!(d.white_level, [16383.0; 3]);
        assert!(d.warnings.is_empty(), "{:?}", d.warnings);
        let m = d.xyz_to_camera.expect("D65 matrix recorded");
        assert!((m[0][0] - 0.7).abs() < 1e-5 && (m[1][0] + 0.2).abs() < 1e-5);
        assert_eq!(d.matrix_white, Some(D65_WHITE));

        // Interior of the flat patch: exact after scaling.
        for y in 1..15 {
            for x in 1..7 {
                let p = d.image.pixel(x, y);
                let want = [expected(4096), expected(8192), expected(12288)];
                for c in 0..3 {
                    assert!(
                        (p[c] - want[c]).abs() < 1e-6,
                        "({x}, {y}) {p:?} vs {want:?}"
                    );
                }
            }
        }
        // Interior of the gradient: a linear ramp interpolates exactly.
        for y in 1..15 {
            for x in 9..15 {
                let p = d.image.pixel(x, y);
                let s = scene(x, y);
                for c in 0..3 {
                    let want = expected(s[c]);
                    assert!((p[c] - want).abs() < 1e-4, "({x}, {y}) {p:?} vs {want}");
                }
            }
        }
    }

    #[test]
    fn same_file_decodes_identically_twice() {
        let dir = TempDir::new("twice");
        let path = dir.path("twice.dng");
        spec(mosaic_from_scene(16, 16, "RGGB", scene)).write(&path);
        let a = decode_raw(&path).unwrap();
        let b = decode_raw(&path).unwrap();
        assert_eq!(a.image.data(), b.image.data());
        assert_eq!(a, b);
    }

    #[test]
    fn two_frames_with_the_same_levels_share_a_scale() {
        // Frame B is frame A with a brighter gradient and a dimmer right
        // edge; the flat patch must decode to the same values, because the
        // scale comes from the levels and not from the frame's own range.
        let dir = TempDir::new("roll");
        let a_path = dir.path("a.dng");
        let b_path = dir.path("b.dng");
        spec(mosaic_from_scene(16, 16, "RGGB", scene)).write(&a_path);
        let brighter = |x: usize, y: usize| {
            if x < 8 {
                scene(x, y)
            } else {
                [20000, 16000, 300]
            }
        };
        spec(mosaic_from_scene(16, 16, "RGGB", brighter)).write(&b_path);

        let a = decode_raw(&a_path).unwrap();
        let b = decode_raw(&b_path).unwrap();
        assert_eq!(a.white_level, b.white_level);
        assert_eq!(a.black_level, b.black_level);
        for y in 1..15 {
            for x in 1..7 {
                assert_eq!(a.image.pixel(x, y), b.image.pixel(x, y), "({x}, {y})");
            }
        }
        // Above the white level stays above 1; below black clips at 0.
        let p = b.image.pixel(12, 8);
        assert!(p[0] > 1.0, "{p:?}");
        assert_eq!(p[2], 0.0);
    }

    #[test]
    fn active_area_crops_and_keeps_the_pattern() {
        let dir = TempDir::new("crop");
        let path = dir.path("crop.dng");
        // Top, left, bottom, right: a 13x11 window starting at an odd offset.
        // DNG defines CFAPattern relative to the active area origin, so a
        // file that says RGGB with its area at (1, 3) holds a BGGR mosaic
        // when sampled from the full sensor origin.
        let mut s = spec(mosaic_from_scene(16, 16, "BGGR", scene));
        s.active_area = Some([3, 1, 14, 14]);
        s.write(&path);
        let d = decode_raw(&path).unwrap();
        assert_eq!((d.image.width(), d.image.height()), (13, 11));
        // Flat region is still exact inside the crop.
        let p = d.image.pixel(2, 2);
        let want = [expected(4096), expected(8192), expected(12288)];
        for c in 0..3 {
            assert!((p[c] - want[c]).abs() < 1e-6, "{p:?} vs {want:?}");
        }
    }

    #[test]
    fn other_patterns_decode_too() {
        for name in ["BGGR", "GRBG", "GBRG"] {
            let dir = TempDir::new(name);
            let path = dir.path("p.dng");
            let mut s = spec(mosaic_from_scene(16, 16, name, scene));
            s.pattern = name;
            s.write(&path);
            let d = decode_raw(&path).unwrap();
            let p = d.image.pixel(3, 3);
            let want = [expected(4096), expected(8192), expected(12288)];
            for c in 0..3 {
                assert!((p[c] - want[c]).abs() < 1e-6, "{name}: {p:?} vs {want:?}");
            }
        }
    }

    #[test]
    fn missing_matrix_gives_none_with_a_warning() {
        let dir = TempDir::new("nomatrix");
        let path = dir.path("nomatrix.dng");
        let mut s = spec(mosaic_from_scene(16, 16, "RGGB", scene));
        s.matrix = None;
        s.write(&path);
        let d = decode_raw(&path).unwrap();
        assert!(d.xyz_to_camera.is_none());
        assert!(d.matrix_white.is_none());
        assert!(
            d.warnings.iter().any(|w| w.contains("no colour matrix")),
            "{:?}",
            d.warnings
        );
    }

    #[test]
    fn missing_file_is_io_and_garbage_is_raw() {
        let dir = TempDir::new("badraw");
        match decode_raw(&dir.path("nope.cr2")) {
            Err(DecodeError::Io(_)) => {}
            other => panic!("expected Io, got {other:?}"),
        }
        let path = dir.path("garbage.cr2");
        std::fs::write(&path, b"definitely not a raw file").unwrap();
        match decode_raw(&path) {
            Err(DecodeError::Raw(_)) | Err(DecodeError::Unsupported(_)) => {}
            other => panic!("expected Raw or Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn pure_helpers_cover_the_edge_cases() {
        let cfa = rawler::CFA::new("RGGB");
        assert_eq!(
            bayer_pattern(&cfa).unwrap().sites(),
            [RED, GREEN, GREEN, BLUE]
        );
        let xtrans = rawler::CFA::new("GGRGGBGGBGGRBRGRBGGGBGGRGGRGGBRBGBRG");
        match bayer_pattern(&xtrans) {
            Err(DecodeError::Unsupported(msg)) => assert!(msg.contains("6x6"), "{msg}"),
            other => panic!("expected Unsupported, got {other:?}"),
        }
        let rgbe = rawler::CFA::new("RGBE");
        assert!(bayer_pattern(&rgbe).is_err());

        let mut warnings = Vec::new();
        assert!(flat_to_3x3(&[1.0; 12], "D65", &mut warnings).is_none());
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn fallback_prefers_daylight_illuminants() {
        let mut order = [
            Illuminant::A,
            Illuminant::Unknown,
            Illuminant::D50,
            Illuminant::Tungsten,
            Illuminant::D55,
        ];
        order.sort_by_key(|&i| (illuminant_rank(i), i));
        assert_eq!(order[0], Illuminant::D55);
        assert_eq!(order[1], Illuminant::D50);
        assert!(illuminant_rank(Illuminant::Unknown) > illuminant_rank(Illuminant::D75));
        assert!(illuminant_rank(Illuminant::A) > illuminant_rank(Illuminant::Daylight));
    }

    #[test]
    fn illuminant_whites_are_the_cie_values() {
        assert_eq!(
            illuminant_white(Illuminant::D65),
            Some([0.95047, 1.0, 1.08883])
        );
        assert_eq!(
            illuminant_white(Illuminant::D50),
            Some([0.96422, 1.0, 0.82521])
        );
        assert_eq!(
            illuminant_white(Illuminant::D55),
            Some([0.95682, 1.0, 0.92149])
        );
        assert_eq!(
            illuminant_white(Illuminant::D75),
            Some([0.94972, 1.0, 1.22638])
        );
        assert_eq!(
            illuminant_white(Illuminant::A),
            Some([1.09850, 1.0, 0.35585])
        );
        for other in [
            Illuminant::Unknown,
            Illuminant::Tungsten,
            Illuminant::Daylight,
        ] {
            assert_eq!(illuminant_white(other), None, "{other:?}");
        }
    }
}
