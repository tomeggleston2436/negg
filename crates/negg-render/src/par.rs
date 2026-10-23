// SPDX-License-Identifier: MPL-2.0

//! Rayon plumbing shared by the per-image functions: split an image into
//! horizontal row bands and run a per-pixel kernel over each band in
//! parallel. Private; the public functions in the other modules call this.

use negg_types::Image;
use negg_types::image::CHANNELS;
use rayon::prelude::*;

/// Target number of floats per band. Small enough that a 24 MP frame splits
/// into hundreds of bands for load balancing, large enough that the rayon
/// overhead per band is negligible.
const FLOATS_PER_BAND: usize = 1 << 16;

/// Runs `kernel` on every pixel of `image` in place, in parallel over row
/// bands. The kernel sees one interleaved RGB triple at a time.
pub(crate) fn for_each_pixel_mut<F>(image: &mut Image, kernel: F)
where
    F: Fn([f32; 3]) -> [f32; 3] + Sync,
{
    let row_len = image.width() * CHANNELS;
    if row_len == 0 || image.height() == 0 {
        return;
    }
    let rows_per_band = (FLOATS_PER_BAND / row_len).max(1);
    let band_len = rows_per_band * row_len;
    image.data_mut().par_chunks_mut(band_len).for_each(|band| {
        for px in band.as_chunks_mut::<CHANNELS>().0.iter_mut() {
            *px = kernel(*px);
        }
    });
}

/// Runs `kernel` on every row of `image` in place, in parallel over row
/// bands. The kernel gets the row index and the row's interleaved RGB
/// floats (`width * 3` of them).
pub(crate) fn for_each_row_mut<F>(image: &mut Image, kernel: F)
where
    F: Fn(usize, &mut [f32]) + Sync,
{
    let row_len = image.width() * CHANNELS;
    if row_len == 0 || image.height() == 0 {
        return;
    }
    let rows_per_band = (FLOATS_PER_BAND / row_len).max(1);
    let band_len = rows_per_band * row_len;
    image
        .data_mut()
        .par_chunks_mut(band_len)
        .enumerate()
        .for_each(|(band, data)| {
            for (i, row) in data.chunks_exact_mut(row_len).enumerate() {
                kernel(band * rows_per_band + i, row);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_visited_once_with_their_index() {
        // 30000 floats per row: two rows per band, so several bands.
        let mut img = Image::new(10_000, 7);
        for_each_row_mut(&mut img, |y, row| {
            for v in row.iter_mut() {
                *v += y as f32;
            }
        });
        for y in 0..7 {
            for x in [0, 4_999, 9_999] {
                assert_eq!(img.pixel(x, y), [y as f32; 3]);
            }
        }
        let mut empty = Image::new(0, 3);
        for_each_row_mut(&mut empty, |_, _| panic!("no rows"));
    }

    #[test]
    fn visits_every_pixel_once() {
        let mut img = Image::new(7, 5);
        for (i, v) in img.data_mut().iter_mut().enumerate() {
            *v = i as f32;
        }
        for_each_pixel_mut(&mut img, |p| [p[0] + 1.0, p[1] + 1.0, p[2] + 1.0]);
        for (i, v) in img.data().iter().enumerate() {
            assert_eq!(*v, i as f32 + 1.0);
        }
    }

    #[test]
    fn empty_images_are_a_no_op() {
        let mut a = Image::new(0, 4);
        let mut b = Image::new(4, 0);
        for_each_pixel_mut(&mut a, |p| p);
        for_each_pixel_mut(&mut b, |p| p);
        assert_eq!(a.data().len(), 0);
        assert_eq!(b.data().len(), 0);
    }
}
