// SPDX-License-Identifier: MPL-2.0

//! Frame geometry: the crop rectangle, the analysis buffer, the quarter-turn
//! rotation and the horizontal flip (Rulings 70 and 90, ADR 0021).
//!
//! [`Geometry`] is the one geometry field of [`crate::Settings`]. Its crop
//! is normalised to the source frame in its decoded orientation, before any
//! rotation or flip, and serves two purposes: it restricts what the step 3
//! to 6 estimators see (the crop grid and the buffered analysis grid of
//! step 3 of `docs/MATHS.md`), and it frames the export, which
//! [`Geometry::apply`] produces after the render and before the output
//! transform (step 8). Applying geometry only moves pixels, so it lives
//! here rather than in `negg-render`.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::image::{CHANNELS, Image};

/// Default analysis buffer: the fraction of the crop's short side trimmed
/// from every side of the crop for analysis only (Ruling 90). Controller
/// choice, TODO(measure).
pub const ANALYSIS_BUFFER_DEFAULT: f32 = 0.05;

/// Largest valid analysis buffer (Ruling 90).
const ANALYSIS_BUFFER_MAX: f32 = 0.2;

/// Smallest valid crop width or height, as a fraction of the frame.
const MIN_CROP_SIDE: f32 = 0.01;

/// Slack allowed on `x + width` and `y + height` beyond 1, so that a crop
/// written back from rounded fractions still validates.
const EDGE_SLACK: f64 = 1e-6;

/// A crop rectangle normalised to the source frame in its decoded
/// orientation: `x` and `width` are fractions of the frame width, `y` and
/// `height` of its height, with the origin at the top-left.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CropRect {
    /// Left edge, as a fraction of the frame width.
    pub x: f32,
    /// Top edge, as a fraction of the frame height.
    pub y: f32,
    /// Width, as a fraction of the frame width; at least 0.01.
    pub width: f32,
    /// Height, as a fraction of the frame height; at least 0.01.
    pub height: f32,
}

impl CropRect {
    /// Checks the rectangle: every field finite, `width` and `height` in
    /// `[0.01, 1]`, `x` and `y` not negative, and `x + width` and
    /// `y + height` at most `1 + 1e-6`.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] naming the rule that failed.
    pub fn validate(&self) -> Result<()> {
        let fields = [
            ("x", self.x),
            ("y", self.y),
            ("width", self.width),
            ("height", self.height),
        ];
        for (name, v) in fields {
            if !v.is_finite() {
                return Err(Error::Invalid(format!("crop {name} {v} must be finite")));
            }
        }
        for (name, v) in [("width", self.width), ("height", self.height)] {
            if !(MIN_CROP_SIDE..=1.0).contains(&v) {
                return Err(Error::Invalid(format!(
                    "crop {name} {v} must be between {MIN_CROP_SIDE} and 1"
                )));
            }
        }
        for (name, v) in [("x", self.x), ("y", self.y)] {
            if v < 0.0 {
                return Err(Error::Invalid(format!(
                    "crop {name} {v} must not be negative"
                )));
            }
        }
        for (name, start, len) in [
            ("x + width", self.x, self.width),
            ("y + height", self.y, self.height),
        ] {
            let end = f64::from(start) + f64::from(len);
            if end > 1.0 + EDGE_SLACK {
                return Err(Error::Invalid(format!(
                    "crop {name} {end} must not exceed 1"
                )));
            }
        }
        Ok(())
    }
}

/// The geometry of one frame (Ruling 70): an optional crop, the analysis
/// buffer, a clockwise quarter-turn rotation and a horizontal flip.
///
/// Every field has a default, so `{}` loads as no crop, the default buffer,
/// no rotation and no flip.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Geometry {
    /// The crop in source orientation; `None` is the whole frame.
    pub crop: Option<CropRect>,
    /// Fraction of the crop's short side trimmed from every side of the crop
    /// for analysis only (Ruling 90). Valid range `[0, 0.2]`; ignored when
    /// `crop` is `None`.
    pub analysis_buffer: f32,
    /// Clockwise rotation in degrees: 0, 90, 180 or 270.
    pub rotate_degrees: u16,
    /// Mirror the rotated image left to right.
    pub flip_horizontal: bool,
}

impl Default for Geometry {
    fn default() -> Self {
        Geometry {
            crop: None,
            analysis_buffer: ANALYSIS_BUFFER_DEFAULT,
            rotate_degrees: 0,
            flip_horizontal: false,
        }
    }
}

/// `v` rounded half away from zero and clamped to `[lo, hi]`, as a pixel
/// index.
fn round_clamp(v: f64, lo: usize, hi: usize) -> usize {
    let r = v.round();
    if r.is_nan() || r <= lo as f64 {
        lo
    } else if r >= hi as f64 {
        hi
    } else {
        r as usize
    }
}

/// One axis of the crop in pixels: `p0 = round(start n)` clamped to
/// `[0, n - 1]` and `p1 = round((start + len) n)` clamped to `[p0 + 1, n]`
/// (Ruling 70). `n` must be at least 1.
fn crop_axis(start: f32, len: f32, n: usize) -> (usize, usize) {
    let nf = n as f64;
    let p0 = round_clamp(f64::from(start) * nf, 0, n - 1);
    let p1 = round_clamp((f64::from(start) + f64::from(len)) * nf, p0 + 1, n);
    (p0, p1)
}

impl Geometry {
    /// Whether this geometry leaves the frame as it is: no crop, no
    /// rotation and no flip. The analysis buffer is irrelevant without a
    /// crop.
    pub fn is_identity(&self) -> bool {
        self.crop.is_none() && self.rotate_degrees == 0 && !self.flip_horizontal
    }

    /// Checks the geometry: `rotate_degrees` is 0, 90, 180 or 270, the crop
    /// (when any) passes [`CropRect::validate`], and `analysis_buffer` is
    /// finite and in `[0, 0.2]`.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] naming the rule that failed.
    pub fn validate(&self) -> Result<()> {
        if !matches!(self.rotate_degrees, 0 | 90 | 180 | 270) {
            return Err(Error::Invalid(format!(
                "rotate_degrees {} must be 0, 90, 180 or 270",
                self.rotate_degrees
            )));
        }
        if let Some(crop) = &self.crop {
            crop.validate()?;
        }
        let b = self.analysis_buffer;
        if !(b.is_finite() && (0.0..=ANALYSIS_BUFFER_MAX).contains(&b)) {
            return Err(Error::Invalid(format!(
                "analysis_buffer {b} must be between 0 and {ANALYSIS_BUFFER_MAX}"
            )));
        }
        Ok(())
    }

    /// The crop in source pixels of a `w` by `h` frame, as
    /// `(x0, y0, x1, y1)` with `x1` and `y1` exclusive (Ruling 70):
    /// `x0 = round(x w)` clamped to `[0, w - 1]`, `x1 = round((x + width) w)`
    /// clamped to `[x0 + 1, w]`, the same for `y`, rounding half away from
    /// zero in `f64`. The whole frame when there is no crop or the frame is
    /// empty.
    pub fn crop_pixels(&self, w: usize, h: usize) -> (usize, usize, usize, usize) {
        match &self.crop {
            Some(c) if w > 0 && h > 0 => {
                let (x0, x1) = crop_axis(c.x, c.width, w);
                let (y0, y1) = crop_axis(c.y, c.height, h);
                (x0, y0, x1, y1)
            }
            _ => (0, 0, w, h),
        }
    }

    /// The analysis rectangle in source pixels (Ruling 90): the crop of
    /// [`Geometry::crop_pixels`] shrunk on each side by
    /// `round(analysis_buffer * min(x1 - x0, y1 - y0))` pixels, limited so
    /// that at least one pixel is left in each direction. The whole frame
    /// when there is no crop (the buffer is then ignored).
    pub fn analysis_pixels(&self, w: usize, h: usize) -> (usize, usize, usize, usize) {
        if self.crop.is_none() {
            return (0, 0, w, h);
        }
        let (x0, y0, x1, y1) = self.crop_pixels(w, h);
        let (cw, ch) = (x1 - x0, y1 - y0);
        let short = cw.min(ch) as f64;
        let buffer = f64::from(self.analysis_buffer.max(0.0));
        let b = round_clamp(buffer * short, 0, usize::MAX);
        let bx = b.min(cw.saturating_sub(1) / 2);
        let by = b.min(ch.saturating_sub(1) / 2);
        (x0 + bx, y0 + by, x1 - bx, y1 - by)
    }

    /// The size of a `w` by `h` frame after the crop, the rotation and the
    /// flip: the crop's size, with width and height swapped for 90 and 270
    /// degrees.
    pub fn output_size(&self, w: usize, h: usize) -> (usize, usize) {
        let (x0, y0, x1, y1) = self.crop_pixels(w, h);
        let (cw, ch) = (x1 - x0, y1 - y0);
        if matches!(self.rotate_degrees, 90 | 270) {
            (ch, cw)
        } else {
            (cw, ch)
        }
    }

    /// Applies the geometry to an image (Ruling 70): the crop of
    /// [`Geometry::crop_pixels`], then the clockwise rotation, then the
    /// horizontal flip of the rotated image. Only moves pixels. An identity
    /// geometry returns an equal copy; callers that want to avoid the copy
    /// check [`Geometry::is_identity`] first.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] when [`Geometry::validate`] fails or the image is
    /// empty while a crop is set.
    pub fn apply(&self, image: &Image) -> Result<Image> {
        self.validate()?;
        let (w, h) = (image.width(), image.height());
        let (x0, y0, x1, y1) = self.crop_pixels(w, h);
        if self.crop.is_some() && (w == 0 || h == 0) {
            return Err(Error::Invalid(format!(
                "cannot crop a {w}x{h} image: it has no pixels"
            )));
        }
        let (cw, ch) = (x1 - x0, y1 - y0);
        let (ow, oh) = self.output_size(w, h);
        let src = image.data();
        let mut out = Vec::with_capacity(ow * oh * CHANNELS);
        for oy in 0..oh {
            for ox in 0..ow {
                // Undo the flip, then the rotation, to find the pixel of the
                // cropped image that lands here.
                let rx = if self.flip_horizontal {
                    ow - 1 - ox
                } else {
                    ox
                };
                let ry = oy;
                let (cx, cy) = match self.rotate_degrees {
                    90 => (ry, ch - 1 - rx),
                    180 => (cw - 1 - rx, ch - 1 - ry),
                    270 => (cw - 1 - ry, rx),
                    _ => (rx, ry),
                };
                let i = ((y0 + cy) * w + x0 + cx) * CHANNELS;
                out.extend_from_slice(&src[i..i + CHANNELS]);
            }
        }
        Image::from_vec(ow, oh, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crop(x: f32, y: f32, width: f32, height: f32) -> Geometry {
        Geometry {
            crop: Some(CropRect {
                x,
                y,
                width,
                height,
            }),
            ..Geometry::default()
        }
    }

    /// A 3 x 2 image whose pixel (x, y) is [x, y, 10 y + x].
    fn three_by_two() -> Image {
        let mut img = Image::new(3, 2);
        for y in 0..2 {
            for x in 0..3 {
                img.set_pixel(x, y, [x as f32, y as f32, (10 * y + x) as f32]);
            }
        }
        img
    }

    /// The third channel of every pixel, row by row: the source index
    /// `10 y + x` the pixel came from.
    fn ids(img: &Image) -> Vec<Vec<u32>> {
        (0..img.height())
            .map(|y| {
                (0..img.width())
                    .map(|x| img.pixel(x, y)[2] as u32)
                    .collect()
            })
            .collect()
    }

    #[test]
    fn default_and_empty_object() {
        let g = Geometry::default();
        assert_eq!(g.crop, None);
        assert_eq!(g.analysis_buffer, ANALYSIS_BUFFER_DEFAULT);
        assert_eq!(g.rotate_degrees, 0);
        assert!(!g.flip_horizontal);
        assert!(g.is_identity());
        g.validate().unwrap();
        let empty: Geometry = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, g);
    }

    #[test]
    fn round_trips_through_json() {
        let g = Geometry {
            crop: Some(CropRect {
                x: 0.1,
                y: 0.2,
                width: 0.5,
                height: 0.6,
            }),
            analysis_buffer: 0.1,
            rotate_degrees: 270,
            flip_horizontal: true,
        };
        let json = serde_json::to_string(&g).unwrap();
        let back: Geometry = serde_json::from_str(&json).unwrap();
        assert_eq!(back, g);
        assert!(!g.is_identity());
        // A crop rect must give every field.
        assert!(serde_json::from_str::<CropRect>(r#"{"x": 0.1}"#).is_err());
    }

    #[test]
    fn validate_rules() {
        crop(0.0, 0.0, 1.0, 1.0).validate().unwrap();
        crop(0.5, 0.5, 0.5, 0.5).validate().unwrap();
        crop(0.99, 0.0, 0.01, 1.0).validate().unwrap();
        for bad in [
            crop(0.5, 0.5, 0.6, 0.1),
            crop(-0.01, 0.0, 0.5, 0.5),
            crop(0.0, 0.0, 0.005, 0.5),
            crop(0.0, 0.0, 0.5, 1.5),
            crop(f32::NAN, 0.0, 0.5, 0.5),
            crop(0.0, 0.0, 0.5, f32::INFINITY),
        ] {
            assert!(matches!(bad.validate(), Err(Error::Invalid(_))), "{bad:?}");
        }
        for rotate in [0, 90, 180, 270] {
            let g = Geometry {
                rotate_degrees: rotate,
                ..Geometry::default()
            };
            g.validate().unwrap();
        }
        for rotate in [45, 360, 1] {
            let g = Geometry {
                rotate_degrees: rotate,
                ..Geometry::default()
            };
            assert!(g.validate().is_err(), "{rotate}");
        }
        for buffer in [-0.01, 0.21, f32::NAN] {
            let g = Geometry {
                analysis_buffer: buffer,
                ..Geometry::default()
            };
            assert!(g.validate().is_err(), "{buffer}");
        }
        for buffer in [0.0, 0.2] {
            let g = Geometry {
                analysis_buffer: buffer,
                ..Geometry::default()
            };
            g.validate().unwrap();
        }
    }

    #[test]
    fn every_rotation_and_flip_on_a_3_by_2_image() {
        let img = three_by_two();
        // Source ids:   0  1  2
        //              10 11 12
        let cases: [(u16, bool, Vec<Vec<u32>>); 8] = [
            (0, false, vec![vec![0, 1, 2], vec![10, 11, 12]]),
            (0, true, vec![vec![2, 1, 0], vec![12, 11, 10]]),
            (90, false, vec![vec![10, 0], vec![11, 1], vec![12, 2]]),
            (90, true, vec![vec![0, 10], vec![1, 11], vec![2, 12]]),
            (180, false, vec![vec![12, 11, 10], vec![2, 1, 0]]),
            (180, true, vec![vec![10, 11, 12], vec![0, 1, 2]]),
            (270, false, vec![vec![2, 12], vec![1, 11], vec![0, 10]]),
            (270, true, vec![vec![12, 2], vec![11, 1], vec![10, 0]]),
        ];
        for (rotate, flip, want) in cases {
            let g = Geometry {
                rotate_degrees: rotate,
                flip_horizontal: flip,
                ..Geometry::default()
            };
            let out = g.apply(&img).unwrap();
            assert_eq!(
                (out.width(), out.height()),
                g.output_size(3, 2),
                "{rotate} {flip}"
            );
            assert_eq!(ids(&out), want, "{rotate} {flip}");
        }
    }

    #[test]
    fn crop_comes_before_the_rotation() {
        let img = three_by_two();
        // Crop to the right two columns (x from 1/3), then rotate 90.
        let mut g = crop(1.0 / 3.0, 0.0, 2.0 / 3.0, 1.0);
        assert_eq!(g.crop_pixels(3, 2), (1, 0, 3, 2));
        g.rotate_degrees = 90;
        let out = g.apply(&img).unwrap();
        assert_eq!(ids(&out), vec![vec![11, 1], vec![12, 2]]);
        assert_eq!(g.output_size(3, 2), (2, 2));
        g.flip_horizontal = true;
        assert_eq!(ids(&g.apply(&img).unwrap()), vec![vec![1, 11], vec![2, 12]]);
    }

    #[test]
    fn identity_apply_is_equal() {
        let img = three_by_two();
        assert_eq!(Geometry::default().apply(&img).unwrap(), img);
        let whole = crop(0.0, 0.0, 1.0, 1.0);
        assert_eq!(whole.apply(&img).unwrap(), img);
    }

    #[test]
    fn crop_rounding_at_half_pixel_boundaries() {
        // 0.125 * 4 = 0.5 rounds away from zero to 1; (0.125 + 0.5) * 4 =
        // 2.5 rounds to 3.
        let g = crop(0.125, 0.125, 0.5, 0.5);
        assert_eq!(g.crop_pixels(4, 4), (1, 1, 3, 3));
        // 0.375 * 4 = 1.5 -> 2; (0.375 + 0.25) * 4 = 2.5 -> 3.
        let g = crop(0.375, 0.0, 0.25, 1.0);
        assert_eq!(g.crop_pixels(4, 4), (2, 0, 3, 4));
        // A sliver that rounds to zero width still keeps one pixel, and a
        // left edge that rounds to the frame width is clamped to w - 1.
        let g = crop(0.99, 0.0, 0.01, 0.01);
        assert_eq!(g.crop_pixels(10, 10), (9, 0, 10, 1));
        // No crop is the whole frame.
        assert_eq!(Geometry::default().crop_pixels(7, 5), (0, 0, 7, 5));
        assert_eq!(Geometry::default().analysis_pixels(7, 5), (0, 0, 7, 5));
    }

    #[test]
    fn analysis_buffer_shrinks_the_crop() {
        let mut g = crop(0.1, 0.2, 0.8, 0.6);
        // Crop of a 1000 x 500 frame: x 100..900, y 100..400; short side
        // 300; buffer 0.05 -> 15 px each side.
        assert_eq!(g.crop_pixels(1000, 500), (100, 100, 900, 400));
        assert_eq!(g.analysis_pixels(1000, 500), (115, 115, 885, 385));
        g.analysis_buffer = 0.0;
        assert_eq!(g.analysis_pixels(1000, 500), (100, 100, 900, 400));
        g.analysis_buffer = 0.2;
        assert_eq!(g.analysis_pixels(1000, 500), (160, 160, 840, 340));
        // A tiny crop keeps at least one pixel in each direction.
        let mut tiny = crop(0.0, 0.0, 0.02, 0.02);
        tiny.analysis_buffer = 0.2;
        let (x0, y0, x1, y1) = tiny.analysis_pixels(100, 100);
        assert!(x1 > x0 && y1 > y0, "{x0} {y0} {x1} {y1}");
        // Without a crop the buffer is ignored.
        let none = Geometry {
            analysis_buffer: 0.2,
            ..Geometry::default()
        };
        assert_eq!(none.analysis_pixels(1000, 500), (0, 0, 1000, 500));
    }

    #[test]
    fn apply_rejects_invalid_geometry() {
        let img = three_by_two();
        let g = Geometry {
            rotate_degrees: 45,
            ..Geometry::default()
        };
        assert!(matches!(g.apply(&img), Err(Error::Invalid(_))));
        assert!(matches!(
            crop(0.5, 0.0, 0.6, 1.0).apply(&img),
            Err(Error::Invalid(_))
        ));
        assert!(crop(0.0, 0.0, 1.0, 1.0).apply(&Image::new(0, 0)).is_err());
    }
}
