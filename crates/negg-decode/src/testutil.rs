// SPDX-License-Identifier: MPL-2.0

//! Test-only helpers: a self-removing temp directory and a writer for
//! minimal synthetic DNGs built with the `tiff` crate's encoder.

use std::fs::File;
use std::path::{Path, PathBuf};

use tiff::encoder::colortype::Gray16;
use tiff::encoder::{SRational, TiffEncoder};
use tiff::tags::Tag;

/// A temp directory under `std::env::temp_dir()` that is removed on drop.
pub(crate) struct TempDir(PathBuf);

impl TempDir {
    pub(crate) fn new(tag: &str) -> TempDir {
        let dir = std::env::temp_dir().join(format!(
            "negg-decode-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    pub(crate) fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Builds a Bayer mosaic by sampling `scene(x, y) -> [r, g, b]` through
/// the named 2x2 pattern (for example `"RGGB"`).
pub(crate) fn mosaic_from_scene(
    width: usize,
    height: usize,
    pattern: &str,
    scene: impl Fn(usize, usize) -> [u16; 3],
) -> Vec<u16> {
    let sites: Vec<usize> = pattern
        .chars()
        .map(|c| match c {
            'R' => 0,
            'G' => 1,
            'B' => 2,
            other => panic!("bad pattern letter {other}"),
        })
        .collect();
    assert_eq!(sites.len(), 4);
    let mut out = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            out.push(scene(x, y)[sites[(y & 1) * 2 + (x & 1)]]);
        }
    }
    out
}

/// Everything a minimal DNG needs: one uncompressed 16-bit CFA IFD with
/// the DNG tags rawler reads.
pub(crate) struct DngSpec {
    pub width: u32,
    pub height: u32,
    pub black: u16,
    pub white: u16,
    /// Four letters from R, G and B, row-major over the 2x2 tile.
    pub pattern: &'static str,
    pub make: &'static str,
    pub model: &'static str,
    /// ColorMatrix2 (XYZ to camera, row-major) under CalibrationIlluminant2 = D65.
    pub matrix: Option<[[f32; 3]; 3]>,
    /// DNG ActiveArea: top, left, bottom, right (exclusive).
    pub active_area: Option<[u32; 4]>,
    /// `width * height` mosaic samples.
    pub data: Vec<u16>,
}

impl DngSpec {
    /// Writes the DNG to `path`.
    pub(crate) fn write(&self, path: &Path) {
        assert_eq!(self.data.len(), (self.width * self.height) as usize);
        let cfa: Vec<u8> = self
            .pattern
            .chars()
            .map(|c| match c {
                'R' => 0u8,
                'G' => 1,
                'B' => 2,
                other => panic!("bad pattern letter {other}"),
            })
            .collect();

        let mut enc = TiffEncoder::new(File::create(path).unwrap()).unwrap();
        let mut img = enc.new_image::<Gray16>(self.width, self.height).unwrap();
        let e = img.encoder();
        // PhotometricInterpretation 32803 = CFA, overriding the Gray16 default.
        e.write_tag(Tag::PhotometricInterpretation, 32803u16)
            .unwrap();
        e.write_tag(Tag::Make, self.make).unwrap();
        e.write_tag(Tag::Model, self.model).unwrap();
        // DNGVersion 1.4.0.0
        e.write_tag(Tag::Unknown(50706), &[1u8, 4, 0, 0][..])
            .unwrap();
        // UniqueCameraModel
        e.write_tag(
            Tag::Unknown(50708),
            format!("{} {}", self.make, self.model).as_str(),
        )
        .unwrap();
        // CFARepeatPatternDim and CFAPattern
        e.write_tag(Tag::Unknown(33421), &[2u16, 2][..]).unwrap();
        e.write_tag(Tag::Unknown(33422), cfa.as_slice()).unwrap();
        // BlackLevel and WhiteLevel
        e.write_tag(Tag::Unknown(50714), &[self.black][..]).unwrap();
        e.write_tag(Tag::Unknown(50717), &[u32::from(self.white)][..])
            .unwrap();
        if let Some(m) = self.matrix {
            // CalibrationIlluminant2 = 21 (D65), ColorMatrix2 as SRATIONAL.
            e.write_tag(Tag::Unknown(50779), 21u16).unwrap();
            let flat: Vec<SRational> = m
                .iter()
                .flatten()
                .map(|&v| SRational {
                    n: (v * 10_000.0).round() as i32,
                    d: 10_000,
                })
                .collect();
            e.write_tag(Tag::Unknown(50722), flat.as_slice()).unwrap();
        }
        if let Some(area) = self.active_area {
            e.write_tag(Tag::Unknown(50829), &area[..]).unwrap();
        }
        img.write_data(&self.data).unwrap();
        drop(enc);
    }
}
