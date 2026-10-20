// SPDX-License-Identifier: MPL-2.0

//! The shared linear image buffer.
//!
//! An [`Image`] is `width * height` pixels of interleaved RGB `f32`, stored
//! row-major with no padding, so a row is a contiguous slice of
//! `width * CHANNELS` floats and the whole image is one `Vec<f32>`. Pixel
//! data is linear and stays `f32` from decode to the final encode (ground
//! rule 4), and it never goes through serde: the type deliberately has no
//! `Serialize` implementation.
//!
//! The iteration helpers ([`Image::rows`], [`Image::row_bands_mut`],
//! [`Image::tiles`]) exist so that `negg-render` can hand contiguous bands to
//! rayon and so that estimators can walk the image tile by tile without
//! knowing the layout.

use crate::error::{Error, Result};

/// Number of interleaved channels per pixel. Always 3 (RGB).
pub const CHANNELS: usize = 3;

/// A linear RGB image of `f32` samples, interleaved and row-major.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    width: usize,
    height: usize,
    data: Vec<f32>,
}

/// A rectangle of pixels inside an [`Image`], produced by [`Image::tiles`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tile {
    /// Column of the tile's left edge, in pixels.
    pub x: usize,
    /// Row of the tile's top edge, in pixels.
    pub y: usize,
    /// Width of the tile, in pixels. Edge tiles may be narrower than asked.
    pub width: usize,
    /// Height of the tile, in pixels. Edge tiles may be shorter than asked.
    pub height: usize,
}

/// Length in floats of a `width * height` image, or `None` on overflow.
fn buffer_len(width: usize, height: usize) -> Option<usize> {
    width.checked_mul(height)?.checked_mul(CHANNELS)
}

impl Image {
    /// Creates a zero-filled image. Either dimension may be 0.
    ///
    /// # Panics
    ///
    /// Panics if `width * height * CHANNELS` overflows `usize`.
    pub fn new(width: usize, height: usize) -> Image {
        let len = buffer_len(width, height).expect("image dimensions overflow usize");
        Image {
            width,
            height,
            data: vec![0.0; len],
        }
    }

    /// Wraps an existing interleaved RGB buffer.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Dimensions`] if `data.len()` is not
    /// `width * height * CHANNELS`.
    pub fn from_vec(width: usize, height: usize, data: Vec<f32>) -> Result<Image> {
        match buffer_len(width, height) {
            Some(len) if len == data.len() => Ok(Image {
                width,
                height,
                data,
            }),
            _ => Err(Error::Dimensions {
                width,
                height,
                len: data.len(),
            }),
        }
    }

    /// Width in pixels.
    pub fn width(&self) -> usize {
        self.width
    }

    /// Height in pixels.
    pub fn height(&self) -> usize {
        self.height
    }

    /// Number of channels per pixel; always [`CHANNELS`].
    pub const fn channels(&self) -> usize {
        CHANNELS
    }

    /// The whole buffer, interleaved RGB, row-major.
    pub fn data(&self) -> &[f32] {
        &self.data
    }

    /// Mutable access to the whole buffer, interleaved RGB, row-major.
    pub fn data_mut(&mut self) -> &mut [f32] {
        &mut self.data
    }

    /// Consumes the image and returns its buffer.
    pub fn into_data(self) -> Vec<f32> {
        self.data
    }

    /// Index into `data` of the first channel of pixel `(x, y)`.
    ///
    /// # Panics
    ///
    /// Panics if `x >= width` or `y >= height`.
    fn offset(&self, x: usize, y: usize) -> usize {
        assert!(
            x < self.width && y < self.height,
            "pixel ({x}, {y}) is outside a {}x{} image",
            self.width,
            self.height
        );
        (y * self.width + x) * CHANNELS
    }

    /// Reads the pixel at column `x`, row `y`.
    ///
    /// # Panics
    ///
    /// Panics if `x >= width` or `y >= height`.
    pub fn pixel(&self, x: usize, y: usize) -> [f32; 3] {
        let i = self.offset(x, y);
        [self.data[i], self.data[i + 1], self.data[i + 2]]
    }

    /// Writes the pixel at column `x`, row `y`.
    ///
    /// # Panics
    ///
    /// Panics if `x >= width` or `y >= height`.
    pub fn set_pixel(&mut self, x: usize, y: usize, rgb: [f32; 3]) {
        let i = self.offset(x, y);
        self.data[i..i + CHANNELS].copy_from_slice(&rgb);
    }

    /// Length of one row in floats.
    fn row_len(&self) -> usize {
        self.width * CHANNELS
    }

    /// Iterates over rows, each a contiguous slice of `width * CHANNELS`
    /// floats. An image with zero width or zero height has no rows.
    pub fn rows(&self) -> impl Iterator<Item = &[f32]> {
        // `chunks` needs a non-zero size; a zero-width image has no data, so
        // any positive size yields nothing.
        self.data.chunks(self.row_len().max(1))
    }

    /// Mutable counterpart of [`Image::rows`].
    pub fn rows_mut(&mut self) -> impl Iterator<Item = &mut [f32]> {
        let row_len = self.row_len().max(1);
        self.data.chunks_mut(row_len)
    }

    /// Splits the image into horizontal bands of up to `rows_per_band` rows,
    /// yielding `(first_row, band)` where `band` is the contiguous slice of
    /// those rows. The last band may be shorter. This is the unit of work
    /// `negg-render` hands to rayon. An image with zero width or zero height
    /// has no bands.
    ///
    /// # Panics
    ///
    /// Panics if `rows_per_band == 0`.
    pub fn row_bands_mut(
        &mut self,
        rows_per_band: usize,
    ) -> impl Iterator<Item = (usize, &mut [f32])> {
        assert!(rows_per_band > 0, "rows_per_band must be at least 1");
        let band_len = (self.row_len() * rows_per_band).max(1);
        self.data
            .chunks_mut(band_len)
            .enumerate()
            .map(move |(i, band)| (i * rows_per_band, band))
    }

    /// Iterates over a row-major grid of `tile` by `tile` rectangles that
    /// cover the image exactly once. Tiles on the right and bottom edges are
    /// clipped to the image. An image with zero width or zero height has no
    /// tiles.
    ///
    /// # Panics
    ///
    /// Panics if `tile == 0`.
    pub fn tiles(&self, tile: usize) -> impl Iterator<Item = Tile> {
        assert!(tile > 0, "tile size must be at least 1");
        let (width, height) = (self.width, self.height);
        let across = width.div_ceil(tile);
        let down = height.div_ceil(tile);
        (0..down).flat_map(move |ty| {
            (0..across).map(move |tx| {
                let x = tx * tile;
                let y = ty * tile;
                Tile {
                    x,
                    y,
                    width: tile.min(width - x),
                    height: tile.min(height - y),
                }
            })
        })
    }

    /// Iterates over the pixels of `t` in row-major order.
    ///
    /// # Panics
    ///
    /// Panics if `t` extends beyond the image.
    pub fn tile_pixels(&self, t: Tile) -> impl Iterator<Item = [f32; 3]> {
        assert!(
            t.x + t.width <= self.width && t.y + t.height <= self.height,
            "tile {t:?} extends beyond a {}x{} image",
            self.width,
            self.height
        );
        let width = self.width;
        (t.y..t.y + t.height).flat_map(move |y| {
            let start = (y * width + t.x) * CHANNELS;
            self.data[start..start + t.width * CHANNELS]
                .chunks_exact(CHANNELS)
                .map(|p| [p[0], p[1], p[2]])
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// A width by height image whose pixel (x, y) is [x, y, x + y].
    fn ramp(width: usize, height: usize) -> Image {
        let mut img = Image::new(width, height);
        for y in 0..height {
            for x in 0..width {
                img.set_pixel(x, y, [x as f32, y as f32, (x + y) as f32]);
            }
        }
        img
    }

    /// Paints every tile of `img.tiles(tile)` into a counter per pixel and
    /// checks each pixel was painted exactly once.
    fn assert_tiles_partition(img: &Image, tile: usize) {
        let mut painted = vec![0u8; img.width() * img.height()];
        for t in img.tiles(tile) {
            assert!(t.width > 0 && t.width <= tile, "{t:?}");
            assert!(t.height > 0 && t.height <= tile, "{t:?}");
            for y in t.y..t.y + t.height {
                for x in t.x..t.x + t.width {
                    painted[y * img.width() + x] += 1;
                }
            }
        }
        assert!(painted.iter().all(|&n| n == 1), "coverage {painted:?}");
    }

    #[test]
    fn from_vec_rejects_wrong_length() {
        let err = Image::from_vec(2, 2, vec![0.0; 11]).unwrap_err();
        match err {
            Error::Dimensions { width, height, len } => {
                assert_eq!((width, height, len), (2, 2, 11));
            }
            other => panic!("unexpected error {other:?}"),
        }
        let ok = Image::from_vec(2, 2, vec![0.0; 12]).unwrap();
        assert_eq!(ok.width(), 2);
        assert_eq!(ok.height(), 2);
        assert_eq!(ok.channels(), 3);
        assert_eq!(ok.data().len(), 12);
    }

    #[test]
    fn empty_image_has_no_rows_or_tiles() {
        let img = Image::new(0, 0);
        assert_eq!(img.rows().count(), 0);
        assert_eq!(img.tiles(16).count(), 0);
        assert_eq!(img.data().len(), 0);
        assert_eq!(img.into_data().len(), 0);

        let mut zero_width = Image::new(0, 5);
        assert_eq!(zero_width.rows().count(), 0);
        assert_eq!(zero_width.rows_mut().count(), 0);
        assert_eq!(zero_width.row_bands_mut(2).count(), 0);
        assert_eq!(zero_width.tiles(4).count(), 0);
    }

    #[test]
    fn tiles_64_on_150_by_100_gives_3_by_2() {
        let img = Image::new(150, 100);
        let tiles: Vec<Tile> = img.tiles(64).collect();
        assert_eq!(tiles.len(), 6);
        assert_eq!(
            tiles[0],
            Tile {
                x: 0,
                y: 0,
                width: 64,
                height: 64
            }
        );
        assert_eq!(
            tiles[2],
            Tile {
                x: 128,
                y: 0,
                width: 22,
                height: 64
            }
        );
        assert_eq!(
            tiles[5],
            Tile {
                x: 128,
                y: 64,
                width: 22,
                height: 36
            }
        );
        assert_tiles_partition(&img, 64);
    }

    #[test]
    fn tile_pixels_match_pixel() {
        let img = ramp(13, 9);
        for t in img.tiles(5) {
            let mut got = img.tile_pixels(t);
            for y in t.y..t.y + t.height {
                for x in t.x..t.x + t.width {
                    assert_eq!(got.next(), Some(img.pixel(x, y)), "({x}, {y})");
                }
            }
            assert_eq!(got.next(), None);
        }
    }

    #[test]
    fn rows_are_contiguous_and_in_order() {
        let img = ramp(4, 3);
        let rows: Vec<&[f32]> = img.rows().collect();
        assert_eq!(rows.len(), 3);
        for (y, row) in rows.iter().enumerate() {
            assert_eq!(row.len(), 12);
            assert_eq!(row[0..3], [0.0, y as f32, y as f32]);
        }
    }

    #[test]
    fn row_bands_of_7_over_20_rows() {
        let mut img = Image::new(5, 20);
        let starts: Vec<(usize, usize)> = img
            .row_bands_mut(7)
            .map(|(first, band)| (first, band.len() / (5 * CHANNELS)))
            .collect();
        assert_eq!(starts, vec![(0, 7), (7, 7), (14, 6)]);

        for (first, band) in img.row_bands_mut(7) {
            band.fill(first as f32);
        }
        assert_eq!(img.pixel(0, 0), [0.0; 3]);
        assert_eq!(img.pixel(4, 6), [0.0; 3]);
        assert_eq!(img.pixel(0, 7), [7.0; 3]);
        assert_eq!(img.pixel(2, 13), [7.0; 3]);
        assert_eq!(img.pixel(0, 14), [14.0; 3]);
        assert_eq!(img.pixel(4, 19), [14.0; 3]);
        assert_eq!(img.data()[7 * 5 * CHANNELS - 1], 0.0);
        assert_eq!(img.data()[7 * 5 * CHANNELS], 7.0);
    }

    #[test]
    fn set_pixel_and_data_mut_agree() {
        let mut img = Image::new(3, 2);
        img.set_pixel(2, 1, [1.0, 2.0, 3.0]);
        assert_eq!(&img.data()[15..18], &[1.0, 2.0, 3.0]);
        img.data_mut()[0] = 9.0;
        assert_eq!(img.pixel(0, 0), [9.0, 0.0, 0.0]);
    }

    #[test]
    #[should_panic(expected = "outside")]
    fn pixel_out_of_range_panics() {
        let img = Image::new(2, 2);
        let _ = img.pixel(2, 0);
    }

    #[test]
    #[should_panic(expected = "tile size")]
    fn tiles_zero_panics() {
        let img = Image::new(2, 2);
        let _ = img.tiles(0).count();
    }

    proptest! {
        #[test]
        fn tiles_partition_exactly(
            width in 0usize..64,
            height in 0usize..64,
            tile in 1usize..32,
        ) {
            let img = Image::new(width, height);
            assert_tiles_partition(&img, tile);
            let expected = if width == 0 || height == 0 {
                0
            } else {
                width.div_ceil(tile) * height.div_ceil(tile)
            };
            prop_assert_eq!(img.tiles(tile).count(), expected);
        }
    }
}
