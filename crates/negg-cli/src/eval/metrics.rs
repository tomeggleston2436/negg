// SPDX-License-Identifier: Apache-2.0

//! The evaluation metrics: CIEDE2000 statistics between two images and
//! grey-card neutrality.
//!
//! Phase 0 assumption: both the render and the reference are
//! sRGB-encoded, so each pixel goes through
//! [`negg_cms::lab::srgb_to_lab`] directly. The ICC profile embedded in a
//! reference TIFF is read by `negg-decode` but ignored here.
//! TODO(phase 1): honour the embedded profile via `negg-cms` once ICC
//! transforms exist, and feed the engine's own output as the render.

use negg_cms::lab::{ciede2000, srgb_to_lab};
use negg_types::Image;
use negg_types::image::CHANNELS;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::corpus::Rect;
use crate::eval::error::EvalError;

/// Summary statistics of the per-pixel CIEDE2000 difference.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DeStats {
    /// Mean over every pixel.
    pub mean: f32,
    /// 95th percentile, nearest rank, over every pixel.
    pub p95: f32,
    /// Largest per-pixel difference.
    pub max: f32,
}

/// Grey-card neutrality: how far the card patch sits from the a* = b* = 0
/// axis.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Neutrality {
    /// Mean of |a*| over the patch.
    pub a_mean_abs: f32,
    /// Mean of |b*| over the patch.
    pub b_mean_abs: f32,
}

impl Neutrality {
    /// The single neutrality score `(|a*| + |b*|) / 2`, as averaged
    /// into the summary and compared by tracked cases.
    pub fn score(self) -> f32 {
        (self.a_mean_abs + self.b_mean_abs) / 2.0
    }
}

/// CIEDE2000 between `render` and `reference`, pixel by pixel, with rows
/// processed in parallel. The p95 is the nearest-rank percentile of a
/// sorted copy of all per-pixel values (not a histogram), so it is exact.
/// An image with no pixels gives all zeros.
///
/// # Errors
///
/// [`EvalError::Mismatch`] (with an empty id; see
/// [`EvalError::for_frame`]) if the dimensions differ.
pub fn de2000_stats(render: &Image, reference: &Image) -> Result<DeStats, EvalError> {
    if render.width() != reference.width() || render.height() != reference.height() {
        return Err(EvalError::Mismatch {
            id: String::new(),
            render: (render.width(), render.height()),
            reference: (reference.width(), reference.height()),
        });
    }
    let row_len = render.width() * CHANNELS;
    if row_len == 0 || render.height() == 0 {
        return Ok(DeStats {
            mean: 0.0,
            p95: 0.0,
            max: 0.0,
        });
    }

    let mut values: Vec<f32> = render
        .data()
        .par_chunks_exact(row_len)
        .zip(reference.data().par_chunks_exact(row_len))
        .flat_map_iter(|(a, b)| {
            a.chunks_exact(CHANNELS)
                .zip(b.chunks_exact(CHANNELS))
                .map(|(p, q)| {
                    ciede2000(
                        srgb_to_lab([p[0], p[1], p[2]]),
                        srgb_to_lab([q[0], q[1], q[2]]),
                    )
                })
                .collect::<Vec<f32>>()
        })
        .collect();

    let n = values.len();
    let sum: f64 = values.iter().map(|&v| f64::from(v)).sum();
    values.sort_unstable_by(f32::total_cmp);
    let rank = ((0.95 * n as f64).ceil() as usize).clamp(1, n);
    Ok(DeStats {
        mean: (sum / n as f64) as f32,
        p95: values[rank - 1],
        max: values[n - 1],
    })
}

/// Mean |a*| and |b*| over the pixels inside `card`, where the rect is
/// given as fractions of the image size: the left and top edges are
/// floored, the right and bottom edges are ceiled, and the result is
/// clamped to the image. An empty patch gives zeros.
pub fn neutrality(image: &Image, card: Rect) -> Neutrality {
    let (w, h) = (image.width(), image.height());
    let x0 = ((card.x * w as f32).floor().max(0.0) as usize).min(w);
    let y0 = ((card.y * h as f32).floor().max(0.0) as usize).min(h);
    let x1 = (((card.x + card.width) * w as f32).ceil().max(0.0) as usize).clamp(x0, w);
    let y1 = (((card.y + card.height) * h as f32).ceil().max(0.0) as usize).clamp(y0, h);
    let count = (x1 - x0) * (y1 - y0);
    if count == 0 {
        return Neutrality {
            a_mean_abs: 0.0,
            b_mean_abs: 0.0,
        };
    }
    let (sum_a, sum_b) = (y0..y1)
        .flat_map(|y| (x0..x1).map(move |x| (x, y)))
        .map(|(x, y)| {
            let lab = srgb_to_lab(image.pixel(x, y));
            (f64::from(lab[1].abs()), f64::from(lab[2].abs()))
        })
        .fold((0.0f64, 0.0f64), |(a, b), (da, db)| (a + da, b + db));
    Neutrality {
        a_mean_abs: (sum_a / count as f64) as f32,
        b_mean_abs: (sum_b / count as f64) as f32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled(w: usize, h: usize, rgb: [f32; 3]) -> Image {
        let mut img = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.set_pixel(x, y, rgb);
            }
        }
        img
    }

    #[test]
    fn identical_images_are_zero() {
        let mut a = Image::new(7, 5);
        for (i, v) in a.data_mut().iter_mut().enumerate() {
            *v = (i % 97) as f32 / 97.0;
        }
        let s = de2000_stats(&a, &a.clone()).unwrap();
        assert_eq!(
            s,
            DeStats {
                mean: 0.0,
                p95: 0.0,
                max: 0.0
            }
        );
    }

    #[test]
    fn one_differing_pixel_sets_max_and_p95() {
        let a = filled(4, 4, [0.5, 0.5, 0.5]);
        let mut b = a.clone();
        b.set_pixel(2, 1, [0.9, 0.2, 0.2]);
        let s = de2000_stats(&a, &b).unwrap();
        assert!(s.max > 0.0);
        // Nearest rank: ceil(0.95 * 16) = 16, so p95 is the largest value.
        assert_eq!(s.p95, s.max);
        assert!((s.mean - s.max / 16.0).abs() < 1e-5, "{s:?}");
        let expected = ciede2000(srgb_to_lab([0.5; 3]), srgb_to_lab([0.9, 0.2, 0.2]));
        assert!((s.max - expected).abs() < 1e-6);
    }

    #[test]
    fn p95_leaves_the_worst_five_percent_out() {
        // 100 pixels: 94 identical, 6 differing by the same amount. Nearest
        // rank 95 lands on a differing pixel; 5 differing would not.
        let a = filled(10, 10, [0.5; 3]);
        let mut b = a.clone();
        for i in 0..6 {
            b.set_pixel(i, 0, [0.6, 0.5, 0.5]);
        }
        assert!(de2000_stats(&a, &b).unwrap().p95 > 0.0);
        let mut c = a.clone();
        for i in 0..5 {
            c.set_pixel(i, 0, [0.6, 0.5, 0.5]);
        }
        assert_eq!(de2000_stats(&a, &c).unwrap().p95, 0.0);
    }

    #[test]
    fn mismatch_is_an_error() {
        let a = Image::new(4, 4);
        let b = Image::new(4, 5);
        match de2000_stats(&a, &b) {
            Err(EvalError::Mismatch {
                id,
                render,
                reference,
            }) => {
                assert!(id.is_empty());
                assert_eq!(render, (4, 4));
                assert_eq!(reference, (4, 5));
            }
            other => panic!("expected Mismatch, got {other:?}"),
        }
    }

    #[test]
    fn empty_image_is_zero() {
        let a = Image::new(0, 3);
        let s = de2000_stats(&a, &a.clone()).unwrap();
        assert_eq!(
            s,
            DeStats {
                mean: 0.0,
                p95: 0.0,
                max: 0.0
            }
        );
    }

    #[test]
    fn neutral_grey_is_neutral() {
        let img = filled(8, 8, [0.46, 0.46, 0.46]);
        let n = neutrality(
            &img,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            },
        );
        assert!(n.a_mean_abs.abs() < 1e-3, "{n:?}");
        assert!(n.b_mean_abs.abs() < 1e-3, "{n:?}");
    }

    #[test]
    fn reddish_image_has_positive_a() {
        let img = filled(8, 8, [0.6, 0.4, 0.4]);
        let n = neutrality(
            &img,
            Rect {
                x: 0.25,
                y: 0.25,
                width: 0.5,
                height: 0.5,
            },
        );
        assert!(n.a_mean_abs > 1.0, "{n:?}");
    }

    #[test]
    fn quarter_rect_on_8x8_covers_exactly_16_pixels() {
        // Colour only the 16 pixels of the centre 4x4 block; everything
        // else is neutral grey. If the rect covered any other pixel the mean
        // would drop below the uniform value; if it missed any it would too.
        let mut img = filled(8, 8, [0.5, 0.5, 0.5]);
        let colour = [0.7, 0.4, 0.4];
        for y in 2..6 {
            for x in 2..6 {
                img.set_pixel(x, y, colour);
            }
        }
        let rect = Rect {
            x: 0.25,
            y: 0.25,
            width: 0.5,
            height: 0.5,
        };
        let n = neutrality(&img, rect);
        let lab = srgb_to_lab(colour);
        assert!(
            (n.a_mean_abs - lab[1].abs()).abs() < 1e-5,
            "{n:?} vs {lab:?}"
        );
        assert!(
            (n.b_mean_abs - lab[2].abs()).abs() < 1e-5,
            "{n:?} vs {lab:?}"
        );

        // Shift one coloured pixel outside the block: the mean must drop by
        // exactly one sixteenth.
        img.set_pixel(2, 2, [0.5; 3]);
        let n2 = neutrality(&img, rect);
        assert!(
            (n2.a_mean_abs - lab[1].abs() * 15.0 / 16.0).abs() < 1e-5,
            "{n2:?}"
        );
    }

    #[test]
    fn rect_is_clamped_to_the_image() {
        let img = filled(4, 4, [0.6, 0.4, 0.4]);
        let n = neutrality(
            &img,
            Rect {
                x: 0.9,
                y: 0.9,
                width: 0.5,
                height: 0.5,
            },
        );
        assert!(n.a_mean_abs > 1.0);
        let empty = neutrality(
            &Image::new(0, 0),
            Rect {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            },
        );
        assert_eq!(empty.a_mean_abs, 0.0);
    }
}
