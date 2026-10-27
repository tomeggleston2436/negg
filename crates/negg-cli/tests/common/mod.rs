// SPDX-License-Identifier: Apache-2.0

//! Helpers shared by the end-to-end tests: a temp directory and a
//! synthetic negative written as a 16-bit RGB TIFF with no ICC profile
//! (so `negg-decode` returns it as `Gamma22Assumed` and the whole
//! pipeline runs on it).

#![allow(dead_code)]

use std::fs::File;
use std::path::{Path, PathBuf};

use tiff::encoder::TiffEncoder;
use tiff::encoder::colortype::RGB16;

/// A temp directory under `std::env::temp_dir()`, removed on drop.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> TempDir {
        // The counter keeps names unique when tests in one process start
        // within the clock's resolution (microseconds on macOS).
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "negg-cli-{tag}-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }

    pub fn write(&self, rel: &str, bytes: &[u8]) {
        let p = self.path(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Writes a `w` by `h` synthetic C-41 style negative as a 16-bit RGB TIFF
/// with no ICC profile: an unexposed border of density `dmin` around an
/// interior whose density ramps from `dmin` to `dmin + gamma` with a
/// checker texture. Transmittance `10^-D` is stored gamma 2.2 encoded,
/// which is what the decoder assumes for an untagged integer TIFF.
pub fn write_negative_tiff(path: &Path, w: usize, h: usize, dmin: [f32; 3], gamma: [f32; 3]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let border = (w.min(h) / 8).max(1);
    let mut data = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let interior = x >= border && x < w - border && y >= border && y < h - border;
            let t = if interior {
                let ramp = (x - border) as f32 / (w - 2 * border) as f32;
                let texture = if (x / 2 + y / 2) % 2 == 0 { 0.0 } else { 0.08 };
                (ramp * 0.9 + texture).min(1.0)
            } else {
                0.0
            };
            for c in 0..3 {
                let transmittance = 10f32.powf(-(dmin[c] + gamma[c] * t));
                let encoded = transmittance.powf(1.0 / 2.2);
                data.push((encoded * 65535.0).round() as u16);
            }
        }
    }
    let mut enc = TiffEncoder::new(File::create(path).unwrap()).unwrap();
    enc.write_image::<RGB16>(w as u32, h as u32, &data).unwrap();
}

/// As [`write_negative_tiff`] but encoded with the sRGB transfer function
/// and tagged with negg's sRGB ICC profile, so `negg-decode` returns it as
/// `Icc` and `negg-cms` linearises it through that profile.
pub fn write_negative_tiff_srgb(path: &Path, w: usize, h: usize, dmin: [f32; 3], gamma: [f32; 3]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let border = (w.min(h) / 8).max(1);
    let mut data = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let interior = x >= border && x < w - border && y >= border && y < h - border;
            let t = if interior {
                let ramp = (x - border) as f32 / (w - 2 * border) as f32;
                let texture = if (x / 2 + y / 2) % 2 == 0 { 0.0 } else { 0.08 };
                (ramp * 0.9 + texture).min(1.0)
            } else {
                0.0
            };
            for c in 0..3 {
                let transmittance = 10f32.powf(-(dmin[c] + gamma[c] * t));
                let encoded = negg_cms::lab::srgb_oetf(transmittance);
                data.push((encoded * 65535.0).round() as u16);
            }
        }
    }
    let opts = negg_io::TiffOptions {
        icc: Some(negg_cms::profile_bytes(negg_types::settings::OutputSpace::Srgb).unwrap()),
        ..negg_io::TiffOptions::default()
    };
    negg_io::write_tiff16(path, w, h, &data, &opts).unwrap();
}

/// Writes a `w` by `h` 16-bit RGB TIFF of uniform mid grey, as a stand-in
/// reference render.
pub fn write_grey_tiff(path: &Path, w: usize, h: usize) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let data = vec![30_000u16; w * h * 3];
    let mut enc = TiffEncoder::new(File::create(path).unwrap()).unwrap();
    enc.write_image::<RGB16>(w as u32, h as u32, &data).unwrap();
}

/// Writes `data` (`w * h * 3` linear values) as a 32-bit float RGB TIFF,
/// which `negg-decode` returns as `Transfer::Linear`.
pub fn write_float_tiff(path: &Path, w: usize, h: usize, data: &[f32]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut enc = TiffEncoder::new(File::create(path).unwrap()).unwrap();
    enc.write_image::<tiff::encoder::colortype::RGB32Float>(w as u32, h as u32, data)
        .unwrap();
}

/// The linear transmittance of [`write_negative_tiff`]'s negative, plus
/// `extra(x, y)` density per channel on top (for grey patches).
pub fn negative_linear(
    w: usize,
    h: usize,
    dmin: [f32; 3],
    gamma: [f32; 3],
    extra: impl Fn(usize, usize) -> Option<[f32; 3]>,
) -> Vec<f32> {
    let border = (w.min(h) / 8).max(1);
    let mut data = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let interior = x >= border && x < w - border && y >= border && y < h - border;
            let t = if interior {
                let ramp = (x - border) as f32 / (w - 2 * border) as f32;
                let texture = if (x / 2 + y / 2) % 2 == 0 { 0.0 } else { 0.08 };
                (ramp * 0.9 + texture).min(1.0)
            } else {
                0.0
            };
            let patch = extra(x, y);
            for c in 0..3 {
                let d = match patch {
                    Some(p) => dmin[c] + p[c],
                    None => dmin[c] + gamma[c] * t,
                };
                data.push(10f32.powf(-d));
            }
        }
    }
    data
}

/// Reads a JSON file.
pub fn read_json(path: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// The file's bytes with the embedded ICC profile's creation date and time
/// (12 bytes ending at the profile's `acsp` signature) zeroed: negg-cms
/// builds the profile at run time and lcms stamps the current second into
/// it, so two runs a second apart differ there and nowhere else.
pub fn without_icc_date(path: &Path) -> Vec<u8> {
    let mut bytes = std::fs::read(path).unwrap();
    let at = bytes
        .windows(4)
        .position(|w| w == b"acsp")
        .expect("an embedded ICC profile");
    bytes[at - 12..at].fill(0);
    bytes
}
