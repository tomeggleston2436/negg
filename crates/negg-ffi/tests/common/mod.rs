// SPDX-License-Identifier: Apache-2.0

//! Shared helpers: a temp directory and a synthetic float TIFF roll.

#![allow(dead_code)]

use std::fs::File;
use std::path::{Path, PathBuf};

use tiff::encoder::TiffEncoder;
use tiff::encoder::colortype::RGB32Float;

/// A temp directory removed on drop.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> TempDir {
        // The counter keeps names unique when tests in one process start
        // within the clock's resolution (microseconds on macOS).
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "negg-ffi-{tag}-{}-{}-{}",
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

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const W: usize = 192;
const H: usize = 128;

/// Linear transmittance of a synthetic negative: an unexposed border of
/// density `dmin`, a ramp interior and a grey patch.
fn negative(dmin: [f32; 3], gamma: [f32; 3], patch_x: usize) -> Vec<f32> {
    let border = (W.min(H) / 8).max(1);
    let mut data = Vec::with_capacity(W * H * 3);
    for y in 0..H {
        for x in 0..W {
            let interior = x >= border && x < W - border && y >= border && y < H - border;
            let t = if interior {
                let ramp = (x - border) as f32 / (W - 2 * border) as f32;
                let texture = if (x / 2 + y / 2) % 2 == 0 { 0.0 } else { 0.08 };
                (ramp * 0.9 + texture).min(1.0)
            } else {
                0.0
            };
            let in_patch = x >= patch_x && x < patch_x + 20 && (40..60).contains(&y);
            for c in 0..3 {
                let d = if in_patch {
                    dmin[c] + 0.3
                } else {
                    dmin[c] + gamma[c] * t
                };
                data.push(10f32.powf(-d));
            }
        }
    }
    data
}

/// Writes a 4-frame synthetic roll into `dir`.
pub fn write_roll(dir: &Path) {
    let frames = [
        ([0.20, 0.50, 0.70], [1.60, 1.70, 1.80]),
        ([0.21, 0.51, 0.71], [1.55, 1.65, 1.80]),
        ([0.22, 0.52, 0.72], [1.60, 1.70, 1.75]),
        ([0.20, 0.51, 0.70], [1.50, 1.60, 1.70]),
    ];
    for (i, (dmin, gamma)) in frames.into_iter().enumerate() {
        let data = negative(dmin, gamma, 40 + 30 * i);
        let mut enc =
            TiffEncoder::new(File::create(dir.join(format!("frame{i}.tif"))).unwrap()).unwrap();
        enc.write_image::<RGB32Float>(W as u32, H as u32, &data)
            .unwrap();
    }
}
