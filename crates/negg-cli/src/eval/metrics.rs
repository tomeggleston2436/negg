// SPDX-License-Identifier: Apache-2.0

//! The evaluation metrics: CIEDE2000 statistics between the engine's
//! render and a reference render, and grey-card neutrality on the render.
//!
//! The render is the engine's output, linear Rec.2020 after step 7 of
//! `docs/MATHS.md`, and goes through
//! [`negg_cms::lab::rec2020_linear_to_lab`]. A reference render is taken
//! to be sRGB-encoded and goes through [`negg_cms::lab::srgb_to_lab`],
//! both when it is compared with the render and when two references are
//! compared with each other ([`reference_distance`]);
//! the ICC profile embedded in a reference TIFF is read by `negg-decode`
//! but not honoured here. TODO(phase 2): linearise references through
//! their embedded profile with `negg-cms`.
//!
//! A reference registered onto the render ([`crate::eval::align`],
//! docs/ADR/0028) is scored by [`aligned_stats`] over the overlap only,
//! resampled bilinearly onto the render's pixel grid, and also described
//! by [`ToneStats`]: L* percentiles (contrast and highlight roll-off),
//! mean chroma (saturation) and the mean a* and b* difference (cast) of
//! the render and the reference, which explain a CIEDE2000 difference
//! rather than only count it.

use negg_cms::lab::{ciede2000, rec2020_linear_to_lab, srgb_to_lab};
use negg_types::Image;
use negg_types::image::CHANNELS;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::corpus::Rect;
use crate::eval::align::{Registration, sample_rgb};
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
    /// The neutrality error Gate 1 bounds ("grey-card neutrality
    /// error is under dE 4"): the chroma of the patch's mean colour,
    /// `sqrt(mean(a*)^2 + mean(b*)^2)` with signed means.
    /// Absent from reports written before this field existed, which read
    /// back as zero.
    #[serde(default)]
    pub de: f32,
}

impl Neutrality {
    /// The single neutrality score `(|a*| + |b*|) / 2`, as averaged
    /// into the summary and compared by tracked cases.
    pub fn score(self) -> f32 {
        (self.a_mean_abs + self.b_mean_abs) / 2.0
    }
}

/// CIEDE2000 between `render` (linear Rec.2020) and `reference`
/// (sRGB-encoded), pixel by pixel, with rows processed in parallel. The
/// p95 is the nearest-rank percentile of a sorted copy of all per-pixel
/// values (not a histogram), so it is exact. An image with no pixels gives
/// all zeros.
///
/// # Errors
///
/// [`EvalError::Mismatch`] (with an empty id; see
/// [`EvalError::for_frame`]) if the dimensions differ.
pub fn de2000_stats(render: &Image, reference: &Image) -> Result<DeStats, EvalError> {
    de2000_with(render, rec2020_linear_to_lab, reference)
}

/// CIEDE2000 between two reference renders, both sRGB-encoded: the
/// per-frame reference-to-reference distance (negadoctor
/// against NegPy), with the same statistics as [`de2000_stats`].
///
/// # Errors
///
/// As [`de2000_stats`].
pub fn reference_distance(a: &Image, b: &Image) -> Result<DeStats, EvalError> {
    de2000_with(a, srgb_to_lab, b)
}

/// The statistics of [`de2000_stats`], with `a` read into L*a*b* by
/// `a_to_lab` and `b` always sRGB-encoded.
fn de2000_with(
    a: &Image,
    a_to_lab: fn([f32; 3]) -> [f32; 3],
    b: &Image,
) -> Result<DeStats, EvalError> {
    if a.width() != b.width() || a.height() != b.height() {
        return Err(EvalError::Mismatch {
            id: String::new(),
            render: (a.width(), a.height()),
            reference: (b.width(), b.height()),
        });
    }
    let row_len = a.width() * CHANNELS;
    if row_len == 0 || a.height() == 0 {
        return Ok(DeStats {
            mean: 0.0,
            p95: 0.0,
            max: 0.0,
        });
    }

    let mut values: Vec<f32> = a
        .data()
        .par_chunks_exact(row_len)
        .zip(b.data().par_chunks_exact(row_len))
        .flat_map_iter(|(ra, rb)| {
            ra.as_chunks::<CHANNELS>()
                .0
                .iter()
                .zip(rb.as_chunks::<CHANNELS>().0)
                .map(|(p, q)| ciede2000(a_to_lab(*p), srgb_to_lab(*q)))
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

/// Tone and colour descriptors of one image over the overlap with
/// another, in CIE L*a*b* (D65).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ToneDescriptors {
    /// L* at the 1st percentile: the shadows.
    pub l_p1: f32,
    /// L* at the 50th percentile: the midtones.
    pub l_p50: f32,
    /// L* at the 99th percentile: the highlights and their roll-off.
    pub l_p99: f32,
    /// Mean chroma C*ab: the saturation.
    pub chroma_mean: f32,
    /// Mean a*.
    pub a_mean: f32,
    /// Mean b*.
    pub b_mean: f32,
}

/// The tone and colour descriptors of the render and a registered
/// reference over their overlap, and the cast between them.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ToneStats {
    /// The engine's render.
    pub render: ToneDescriptors,
    /// The reference, resampled onto the render's grid.
    pub reference: ToneDescriptors,
    /// Mean a* of the render minus mean a* of the reference: positive is a
    /// render more magenta than the reference.
    pub a_diff: f32,
    /// Mean b* of the render minus mean b* of the reference: positive is a
    /// render more yellow than the reference.
    pub b_diff: f32,
}

/// Width of an L* histogram bin for the percentiles of [`ToneStats`].
const L_BIN: f32 = 0.01;
/// Number of L* bins, covering `[0, 120)`; values outside are clamped
/// into the end bins (a linear render may exceed L* 100).
const L_BINS: usize = 12_000;
/// Rows per band in the parallel pass of [`aligned_stats`].
const BAND_ROWS: usize = 32;

/// Per-band accumulators of [`aligned_stats`].
struct Band {
    de: Vec<f32>,
    hist: [Vec<u32>; 2],
    /// Sums of C*, a*, b* for the render then the reference.
    sums: [f64; 6],
}

fn l_bin(l: f32) -> usize {
    let b = (l / L_BIN).floor();
    if b.is_nan() || b < 0.0 {
        0
    } else {
        (b as usize).min(L_BINS - 1)
    }
}

/// The nearest-rank percentile `p` (in `(0, 1]`) of a histogram of `n`
/// values, as the centre of its bin.
fn hist_percentile(hist: &[u32], n: usize, p: f64) -> f32 {
    let rank = ((p * n as f64).ceil() as usize).clamp(1, n.max(1));
    let mut seen = 0usize;
    for (i, &count) in hist.iter().enumerate() {
        seen += count as usize;
        if seen >= rank {
            return (i as f32 + 0.5) * L_BIN;
        }
    }
    0.0
}

/// CIEDE2000 between `render` (linear Rec.2020) and `reference`
/// (sRGB-encoded) over the overlap of `registration`, with the reference
/// sampled bilinearly at the point each render pixel centre sees, and the
/// [`ToneStats`] of the same pixels. The statistics are those of
/// [`de2000_stats`]; the L* percentiles are nearest-rank on a histogram
/// with bins of 0.01. Rows are processed in parallel in fixed bands
/// summed in order, so the result does not depend on the thread count.
///
/// # Errors
///
/// [`EvalError::Mismatch`] (with an empty id) when the registration has
/// no overlap with the render.
pub fn aligned_stats(
    render: &Image,
    reference: &Image,
    registration: &Registration,
) -> Result<(DeStats, ToneStats), EvalError> {
    let Some((x0, y0, x1, y1)) = registration.overlap_rect(render.width(), render.height()) else {
        return Err(EvalError::Mismatch {
            id: String::new(),
            render: (render.width(), render.height()),
            reference: (reference.width(), reference.height()),
        });
    };
    let starts: Vec<usize> = (y0..y1).step_by(BAND_ROWS).collect();
    let bands: Vec<Band> = starts
        .par_iter()
        .map(|&start| {
            let mut band = Band {
                de: Vec::with_capacity(BAND_ROWS * (x1 - x0)),
                hist: [vec![0u32; L_BINS], vec![0u32; L_BINS]],
                sums: [0.0; 6],
            };
            for y in start..(start + BAND_ROWS).min(y1) {
                for x in x0..x1 {
                    let lab_r = rec2020_linear_to_lab(render.pixel(x, y));
                    let (p, q) = registration.to_reference(x as f64, y as f64);
                    let lab_f = srgb_to_lab(sample_rgb(reference, p, q));
                    band.de.push(ciede2000(lab_r, lab_f));
                    band.hist[0][l_bin(lab_r[0])] += 1;
                    band.hist[1][l_bin(lab_f[0])] += 1;
                    for (k, lab) in [lab_r, lab_f].iter().enumerate() {
                        let (a, b) = (f64::from(lab[1]), f64::from(lab[2]));
                        band.sums[3 * k] += a.hypot(b);
                        band.sums[3 * k + 1] += a;
                        band.sums[3 * k + 2] += b;
                    }
                }
            }
            band
        })
        .collect();

    let n = (x1 - x0) * (y1 - y0);
    let mut values = Vec::with_capacity(n);
    let mut hist = [vec![0u32; L_BINS], vec![0u32; L_BINS]];
    let mut sums = [0.0f64; 6];
    for band in bands {
        values.extend_from_slice(&band.de);
        for (total, part) in hist.iter_mut().zip(&band.hist) {
            for (t, v) in total.iter_mut().zip(part) {
                *t += v;
            }
        }
        for (t, v) in sums.iter_mut().zip(band.sums) {
            *t += v;
        }
    }
    let sum: f64 = values.iter().map(|&v| f64::from(v)).sum();
    values.sort_unstable_by(f32::total_cmp);
    let rank = ((0.95 * n as f64).ceil() as usize).clamp(1, n);
    let de = DeStats {
        mean: (sum / n as f64) as f32,
        p95: values[rank - 1],
        max: values[n - 1],
    };
    let nf = n as f64;
    let describe = |k: usize| ToneDescriptors {
        l_p1: hist_percentile(&hist[k], n, 0.01),
        l_p50: hist_percentile(&hist[k], n, 0.50),
        l_p99: hist_percentile(&hist[k], n, 0.99),
        chroma_mean: (sums[3 * k] / nf) as f32,
        a_mean: (sums[3 * k + 1] / nf) as f32,
        b_mean: (sums[3 * k + 2] / nf) as f32,
    };
    let tone = ToneStats {
        render: describe(0),
        reference: describe(1),
        a_diff: ((sums[1] - sums[4]) / nf) as f32,
        b_diff: ((sums[2] - sums[5]) / nf) as f32,
    };
    Ok((de, tone))
}

/// Mean |a*| and |b*| over the pixels of the render (linear Rec.2020)
/// inside `card`, and the chroma of the patch's mean colour
/// ([`Neutrality::de`]), where the rect is given as fractions of the image
/// size: the left and top edges are floored, the right and bottom edges
/// are ceiled, and the result is clamped to the image. An empty patch
/// gives zeros.
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
            de: 0.0,
        };
    }
    let sums = (y0..y1)
        .flat_map(|y| (x0..x1).map(move |x| (x, y)))
        .map(|(x, y)| {
            let lab = rec2020_linear_to_lab(image.pixel(x, y));
            let (a, b) = (f64::from(lab[1]), f64::from(lab[2]));
            [a.abs(), b.abs(), a, b]
        })
        .fold([0.0f64; 4], |acc, v| {
            [acc[0] + v[0], acc[1] + v[1], acc[2] + v[2], acc[3] + v[3]]
        });
    let n = count as f64;
    let (mean_a, mean_b) = (sums[2] / n, sums[3] / n);
    Neutrality {
        a_mean_abs: (sums[0] / n) as f32,
        b_mean_abs: (sums[1] / n) as f32,
        de: mean_a.hypot(mean_b) as f32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use negg_cms::lab::srgb_oetf;

    fn filled(w: usize, h: usize, rgb: [f32; 3]) -> Image {
        let mut img = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.set_pixel(x, y, rgb);
            }
        }
        img
    }

    /// A grey ramp as linear Rec.2020 and the same greys sRGB-encoded.
    /// Both spaces share D65, so each grey has the same L*a*b* in both.
    fn grey_ramp() -> (Image, Image) {
        let mut linear = Image::new(7, 5);
        let mut encoded = Image::new(7, 5);
        for (i, (l, e)) in linear
            .data_mut()
            .as_chunks_mut::<3>()
            .0
            .iter_mut()
            .zip(encoded.data_mut().as_chunks_mut::<3>().0)
            .enumerate()
        {
            let g = (i % 31) as f32 / 31.0;
            *l = [g; 3];
            *e = [srgb_oetf(g); 3];
        }
        (linear, encoded)
    }

    #[test]
    fn matching_greys_are_zero() {
        let (linear, encoded) = grey_ramp();
        let s = de2000_stats(&linear, &encoded).unwrap();
        assert!(s.max < 2e-3, "{s:?}");
        assert!(s.mean < 2e-3, "{s:?}");
        assert!(s.p95 <= s.max);
    }

    #[test]
    fn one_differing_pixel_sets_max_and_p95() {
        let a = filled(4, 4, [0.5, 0.5, 0.5]);
        let mut b = filled(4, 4, [srgb_oetf(0.5); 3]);
        b.set_pixel(2, 1, [0.9, 0.2, 0.2]);
        let s = de2000_stats(&a, &b).unwrap();
        assert!(s.max > 0.0);
        // Nearest rank: ceil(0.95 * 16) = 16, so p95 is the largest value.
        assert_eq!(s.p95, s.max);
        assert!((s.mean - s.max / 16.0).abs() < 1e-3, "{s:?}");
        let expected = ciede2000(
            rec2020_linear_to_lab([0.5; 3]),
            srgb_to_lab([0.9, 0.2, 0.2]),
        );
        assert!((s.max - expected).abs() < 1e-6);
    }

    #[test]
    fn p95_leaves_the_worst_five_percent_out() {
        // 100 pixels: 94 matching, 6 differing by the same amount. Nearest
        // rank 95 lands on a differing pixel; 5 differing would not.
        let a = filled(10, 10, [0.5; 3]);
        let same = [srgb_oetf(0.5); 3];
        let mut b = filled(10, 10, same);
        for i in 0..6 {
            b.set_pixel(i, 0, [0.9, 0.5, 0.5]);
        }
        assert!(de2000_stats(&a, &b).unwrap().p95 > 0.1);
        let mut c = filled(10, 10, same);
        for i in 0..5 {
            c.set_pixel(i, 0, [0.9, 0.5, 0.5]);
        }
        assert!(de2000_stats(&a, &c).unwrap().p95 < 2e-3);
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
        assert_eq!(s.max, 0.0);
        let s = de2000_stats(&Image::new(3, 0), &Image::new(3, 0)).unwrap();
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
        assert!(n.de < 1e-3, "{n:?}");
    }

    #[test]
    fn neutrality_de_is_the_chroma_of_the_mean_colour() {
        // Half the patch +a*, half -a* of the same size: |a*| is large but
        // the mean colour is neutral, so de is near zero.
        let mut img = filled(8, 8, [0.6, 0.4, 0.4]);
        let lab_red = rec2020_linear_to_lab([0.6, 0.4, 0.4]);
        let whole = Rect {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        };
        let n = neutrality(&img, whole);
        assert!(
            (n.de - lab_red[1].hypot(lab_red[2])).abs() < 1e-4,
            "{n:?} vs {lab_red:?}"
        );
        // Half red, half cyan: the mean of |a*| is large but the signed
        // means partly cancel, and de is the chroma of the signed means.
        let cyan = [0.4, 0.6, 0.6];
        for y in 0..8 {
            for x in 0..4 {
                img.set_pixel(x, y, cyan);
            }
        }
        let lab_cyan = rec2020_linear_to_lab(cyan);
        let n = neutrality(&img, whole);
        let want = ((lab_red[1] + lab_cyan[1]) / 2.0).hypot((lab_red[2] + lab_cyan[2]) / 2.0);
        assert!((n.de - want).abs() < 1e-4, "{n:?} want {want}");
        assert!(n.de < n.a_mean_abs, "{n:?}");
    }

    #[test]
    fn reference_distance_compares_two_srgb_images() {
        let (_, encoded) = grey_ramp();
        let s = reference_distance(&encoded, &encoded.clone()).unwrap();
        assert_eq!(s.max, 0.0);
        let mut other = encoded.clone();
        other.set_pixel(1, 1, [0.9, 0.2, 0.2]);
        let s = reference_distance(&encoded, &other).unwrap();
        let expected = ciede2000(
            srgb_to_lab(encoded.pixel(1, 1)),
            srgb_to_lab([0.9, 0.2, 0.2]),
        );
        assert!((s.max - expected).abs() < 1e-6, "{s:?}");
        assert!(matches!(
            reference_distance(&Image::new(2, 2), &Image::new(3, 2)),
            Err(EvalError::Mismatch { .. })
        ));
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
        let lab = rec2020_linear_to_lab(colour);
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
