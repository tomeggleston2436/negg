// SPDX-License-Identifier: MPL-2.0

//! The preview pyramid of Step 8 of docs/MATHS.md (preview), Ruling 73
//! (R-preview, ADR 0022).
//!
//! The open frame keeps its full linear image as level 0; levels 1 to L
//! are successive halvings by [`downscale_half`], a 2 x 2 box mean in
//! linear light, down to the first level that fits one tile. Preview tiles
//! of level `l` are [`crate::render_region`] of that level with the frame's
//! [`crate::FramePlan`]. Levels above 0 are a preview approximation (a
//! render of a downscale, not a downscale of a render); nothing here feeds
//! an export.

use negg_types::Image;
use negg_types::image::CHANNELS;

use crate::par::for_each_row_mut;

/// Side of a square preview tile in pixels (Ruling 73, `TILE_SIZE`): the
/// pyramid stops at the first level whose long side is at most this, and
/// a tile covers at most this many pixels in each direction. Controller
/// choice.
pub const TILE_SIZE: usize = 512;

/// One pyramid step of Step 8 of docs/MATHS.md (preview), Ruling 73: the
/// 2 x 2 box mean of `image` in linear light, `ceil(w / 2)` by
/// `ceil(h / 2)` pixels.
///
/// Output pixel `(x, y)` is the mean of the source pixels
/// `(2x .. 2x + 1, 2y .. 2y + 1)` that exist: four in the interior, two on
/// an odd last column or row, one at an odd corner. Each channel is summed
/// pairwise, `((a + b) + (c + d)) * 0.25` (or `(a + b) * 0.5`), so a
/// constant image stays exactly constant. An empty image gives an empty
/// image of the halved size.
pub fn downscale_half(image: &Image) -> Image {
    let (w, h) = (image.width(), image.height());
    let (ow, oh) = (w.div_ceil(2), h.div_ceil(2));
    let mut out = Image::new(ow, oh);
    let src = image.data();
    let row_len = w * CHANNELS;
    for_each_row_mut(&mut out, |y, row| {
        let top = &src[2 * y * row_len..(2 * y + 1) * row_len];
        let bottom = (2 * y + 1 < h).then(|| &src[(2 * y + 1) * row_len..(2 * y + 2) * row_len]);
        for (x, px) in row.as_chunks_mut::<CHANNELS>().0.iter_mut().enumerate() {
            let l = 2 * x * CHANNELS;
            let has_right = 2 * x + 1 < w;
            for c in 0..CHANNELS {
                let a = top[l + c];
                px[c] = match (has_right, bottom) {
                    (true, Some(b)) => {
                        ((a + top[l + CHANNELS + c]) + (b[l + c] + b[l + CHANNELS + c])) * 0.25
                    }
                    (true, None) => (a + top[l + CHANNELS + c]) * 0.5,
                    (false, Some(b)) => (a + b[l + c]) * 0.5,
                    (false, None) => a,
                };
            }
        }
    });
    out
}

/// The preview pyramid of Step 8 of docs/MATHS.md (preview), Ruling 73:
/// levels 1 to L of `level0`, each [`downscale_half`] of the one before,
/// where L is the smallest `n` with `max(w_n, h_n) <= tile`. Level 0 is
/// the caller's own image and is not copied; the result is empty when
/// `level0` already fits one tile. An empty `level0` (zero width or
/// height) has nothing to preview and gives no levels, whatever its other
/// side. A `tile` of 0 is treated as 1, so the loop always ends.
pub fn pyramid(level0: &Image, tile: usize) -> Vec<Image> {
    let tile = tile.max(1);
    if level0.width() == 0 || level0.height() == 0 {
        return Vec::new();
    }
    let mut levels: Vec<Image> = Vec::new();
    loop {
        let current = levels.last().unwrap_or(level0);
        if current.width().max(current.height()) <= tile {
            return levels;
        }
        let next = downscale_half(current);
        levels.push(next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbered(w: usize, h: usize) -> Image {
        let mut img = Image::new(w, h);
        for (i, v) in img.data_mut().iter_mut().enumerate() {
            *v = i as f32;
        }
        img
    }

    #[test]
    fn a_constant_stays_exactly_constant_at_any_size() {
        for (w, h) in [(8, 6), (7, 5), (1, 9), (9, 1), (1, 1), (2, 3)] {
            let mut img = Image::new(w, h);
            for px in img.data_mut().as_chunks_mut::<3>().0 {
                *px = [0.123_456_7, 3.0e-6, 0.77];
            }
            let half = downscale_half(&img);
            assert_eq!(half.width(), w.div_ceil(2));
            assert_eq!(half.height(), h.div_ceil(2));
            for px in half.data().as_chunks::<3>().0 {
                assert_eq!(*px, [0.123_456_7, 3.0e-6, 0.77], "{w}x{h}");
            }
        }
    }

    #[test]
    fn odd_edges_average_the_pixels_that_exist() {
        // 3 x 3, one value per pixel in every channel: 0 1 2 / 3 4 5 / 6 7 8.
        let mut img = Image::new(3, 3);
        for y in 0..3 {
            for x in 0..3 {
                img.set_pixel(x, y, [(y * 3 + x) as f32; 3]);
            }
        }
        let half = downscale_half(&img);
        assert_eq!((half.width(), half.height()), (2, 2));
        assert_eq!(half.pixel(0, 0), [2.0; 3]); // (0 + 1 + 3 + 4) / 4
        assert_eq!(half.pixel(1, 0), [3.5; 3]); // (2 + 5) / 2
        assert_eq!(half.pixel(0, 1), [6.5; 3]); // (6 + 7) / 2
        assert_eq!(half.pixel(1, 1), [8.0; 3]); // 8 alone
    }

    #[test]
    fn channels_are_kept_apart() {
        let img = numbered(4, 2);
        let half = downscale_half(&img);
        // Pixel (0, 0) averages pixels 0, 1, 4 and 5: floats 3p + c.
        for c in 0..3 {
            let want = ((c as f32 + (3 + c) as f32) + ((12 + c) as f32 + (15 + c) as f32)) * 0.25;
            assert_eq!(half.pixel(0, 0)[c], want);
        }
    }

    #[test]
    fn empty_images_halve_to_empty_images() {
        let half = downscale_half(&Image::new(0, 5));
        assert_eq!((half.width(), half.height()), (0, 3));
        let half = downscale_half(&Image::new(4, 0));
        assert_eq!((half.width(), half.height()), (2, 0));
    }

    #[test]
    fn level_counts_follow_the_ruling() {
        // 6000 x 4000: 3000, 1500, 750, 375; L = 4.
        let levels = pyramid(&Image::new(6000, 4000), TILE_SIZE);
        let sizes: Vec<_> = levels.iter().map(|l| (l.width(), l.height())).collect();
        assert_eq!(
            sizes,
            vec![(3000, 2000), (1500, 1000), (750, 500), (375, 250)]
        );
        // 512 x 300 already fits a tile: L = 0.
        assert!(pyramid(&Image::new(512, 300), TILE_SIZE).is_empty());
        // 513 x 1 needs one level, rounding up: 257 x 1.
        let one = pyramid(&Image::new(513, 1), TILE_SIZE);
        assert_eq!(one.len(), 1);
        assert_eq!((one[0].width(), one[0].height()), (257, 1));
        // Odd sizes round up at every level: 1025 -> 513 -> 257.
        let odd = pyramid(&Image::new(1025, 3), TILE_SIZE);
        let sizes: Vec<_> = odd.iter().map(|l| (l.width(), l.height())).collect();
        assert_eq!(sizes, vec![(513, 2), (257, 1)]);
        // Empty images give no levels.
        assert!(pyramid(&Image::new(0, 5000), TILE_SIZE).is_empty());
        assert!(pyramid(&Image::new(0, 0), TILE_SIZE).is_empty());
        // A zero tile is treated as one and still terminates.
        assert_eq!(pyramid(&Image::new(5, 3), 0).len(), 3);
    }

    #[test]
    fn each_level_is_the_halving_of_the_one_before() {
        let img = numbered(1100, 37);
        let levels = pyramid(&img, TILE_SIZE);
        assert_eq!(levels.len(), 2);
        assert_eq!(levels[0], downscale_half(&img));
        assert_eq!(levels[1], downscale_half(&levels[0]));
    }
}
