// SPDX-License-Identifier: MPL-2.0

//! Area-average downscale in linear light, for the web export.

use crate::par::for_each_row_mut;
use negg_types::Image;
use negg_types::image::CHANNELS;

/// One output sample's footprint along one axis: the first input index and
/// the normalised weights of the consecutive input samples it covers.
struct Span {
    start: usize,
    weights: Vec<f64>,
}

/// Box-filter spans for shrinking `input` samples to `output` samples. Output
/// `o` covers `[o * s, (o + 1) * s)` with `s = input / output`; each input
/// sample is weighted by its overlap with that interval, so the edge samples
/// carry fractional weight.
fn spans(input: usize, output: usize) -> Vec<Span> {
    let scale = input as f64 / output as f64;
    (0..output)
        .map(|o| {
            let lo = o as f64 * scale;
            let hi = ((o + 1) as f64 * scale).min(input as f64);
            let start = (lo.floor() as usize).min(input - 1);
            let end = (hi.ceil() as usize).clamp(start + 1, input);
            let mut weights: Vec<f64> = (start..end)
                .map(|i| ((i + 1) as f64).min(hi) - (i as f64).max(lo))
                .map(|w| w.max(0.0))
                .collect();
            let total: f64 = weights.iter().sum();
            for w in &mut weights {
                *w /= total;
            }
            Span { start, weights }
        })
        .collect()
}

/// Area-average downscale to a given long edge, never upscaling.
///
/// When `max(w, h) <= long_edge`, or `long_edge` is 0, the image is returned
/// unchanged. Otherwise the scale is `t = long_edge / max(w, h)` and the
/// output is `max(1, round(w t))` by `max(1, round(h t))`, with the long side
/// exactly `long_edge`. Each output pixel is the mean of the input pixels its
/// footprint covers, weighted by overlap area (a box filter with fractional
/// edge weights), accumulated in `f64` in a fixed row-then-column order per
/// output pixel, so the result is identical across runs and thread counts.
/// The data is linear light and no value is clamped or quantised. No ringing,
/// and exact box means at integer ratios.
///
/// Implements the web output paragraph of
/// `docs/MATHS.md#step-8-export` (ADR 0035).
pub fn downscale_to_long_edge(image: &Image, long_edge: usize) -> Image {
    let (w, h) = (image.width(), image.height());
    let long = w.max(h);
    if long_edge == 0 || long <= long_edge || w == 0 || h == 0 {
        return image.clone();
    }
    let t = long_edge as f64 / long as f64;
    let scaled = |n: usize| -> usize {
        if n == long {
            long_edge
        } else {
            (((n as f64) * t).round() as usize).clamp(1, long_edge)
        }
    };
    let (ow, oh) = (scaled(w), scaled(h));
    let xs = spans(w, ow);
    let ys = spans(h, oh);
    let src = image.data();
    let mut out = Image::new(ow, oh);
    for_each_row_mut(&mut out, |oy, row| {
        let ys = &ys[oy];
        for (ox, px) in row.as_chunks_mut::<CHANNELS>().0.iter_mut().enumerate() {
            let xs = &xs[ox];
            let mut acc = [0.0f64; CHANNELS];
            for (j, wy) in ys.weights.iter().enumerate() {
                let base = (ys.start + j) * w;
                for (i, wx) in xs.weights.iter().enumerate() {
                    let wgt = wy * wx;
                    let at = (base + xs.start + i) * CHANNELS;
                    for c in 0..CHANNELS {
                        acc[c] += wgt * src[at + c] as f64;
                    }
                }
            }
            for c in 0..CHANNELS {
                px[c] = acc[c] as f32;
            }
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn random_image(w: usize, h: usize, seed: u32) -> Image {
        let mut s = seed;
        let data = (0..w * h * 3)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                (s >> 8) as f32 / (1u32 << 24) as f32
            })
            .collect();
        Image::from_vec(w, h, data).unwrap()
    }

    #[test]
    fn web_never_upscales() {
        let img = random_image(1000, 600, 7);
        assert_eq!(downscale_to_long_edge(&img, 2048), img);
        assert_eq!(downscale_to_long_edge(&img, 0), img);
        assert_eq!(downscale_to_long_edge(&img, 1000), img);
    }

    #[test]
    fn long_edge_is_exact_and_aspect_kept() {
        let a = downscale_to_long_edge(&Image::new(6000, 4000), 2048);
        assert_eq!((a.width(), a.height()), (2048, 1365));
        let b = downscale_to_long_edge(&Image::new(4000, 6000), 2048);
        assert_eq!((b.width(), b.height()), (1365, 2048));
        let thin = downscale_to_long_edge(&Image::new(5000, 1), 100);
        assert_eq!((thin.width(), thin.height()), (100, 1));
    }

    #[test]
    fn flat_image_stays_flat() {
        let mut img = Image::new(517, 311);
        for y in 0..311 {
            for x in 0..517 {
                img.set_pixel(x, y, [0.3, 0.5, 0.7]);
            }
        }
        let out = downscale_to_long_edge(&img, 100);
        for px in out.data().as_chunks::<3>().0 {
            for (v, e) in px.iter().zip([0.3f32, 0.5, 0.7]) {
                assert!((v - e).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn integer_ratio_equals_box_mean() {
        let img = random_image(8, 4, 99);
        let out = downscale_to_long_edge(&img, 4);
        assert_eq!((out.width(), out.height()), (4, 2));
        for oy in 0..2 {
            for ox in 0..4 {
                for c in 0..3 {
                    let mut sum = 0.0f64;
                    for dy in 0..2 {
                        for dx in 0..2 {
                            sum += img.pixel(ox * 2 + dx, oy * 2 + dy)[c] as f64;
                        }
                    }
                    let want = (sum / 4.0) as f32;
                    assert!((out.pixel(ox, oy)[c] - want).abs() < 1e-6);
                }
            }
        }
    }

    #[test]
    fn energy_is_preserved() {
        let img = random_image(997, 613, 0xC0FFEE);
        let out = downscale_to_long_edge(&img, 300);
        assert_eq!(out.width(), 300);
        let mean =
            |i: &Image| i.data().iter().map(|&v| v as f64).sum::<f64>() / i.data().len() as f64;
        assert!((mean(&img) - mean(&out)).abs() < 1e-5);
    }

    #[test]
    fn identical_across_thread_counts() {
        let img = random_image(401, 263, 5);
        let run = |n: usize| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(n)
                .build()
                .unwrap()
                .install(|| downscale_to_long_edge(&img, 97))
        };
        assert_eq!(run(1), run(4));
    }
}
