// SPDX-License-Identifier: MPL-2.0

//! Bilinear demosaic for 2x2 Bayer colour filter arrays.
//!
//! Phase 1 uses the simplest interpolation that is correct: every output
//! pixel keeps the sample its own filter recorded and takes, for each of
//! the other two colours, the mean of the neighbours of that colour inside
//! its 3x3 window. Interior pixels therefore average four green or four
//! diagonal samples at a red or blue site, and two samples per missing
//! colour at a green site. Edge pixels average whichever of those
//! neighbours exist; a colour with no neighbour at all (only possible in
//! images narrower or shorter than two pixels) comes out as 0.
//!
//! No white balance, colour matrix or gamma is applied here: the output is
//! the mosaic's values spread into three channels and nothing else.

use negg_types::Image;
use negg_types::image::CHANNELS;

use crate::error::DecodeError;

/// Colour index of a red filter site.
pub const RED: u8 = 0;
/// Colour index of a green filter site.
pub const GREEN: u8 = 1;
/// Colour index of a blue filter site.
pub const BLUE: u8 = 2;

/// A 2x2 Bayer pattern: the colour of each of the four sites, row-major,
/// so `[R, G, G, B]` is the common "RGGB" layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BayerPattern {
    sites: [u8; 4],
}

impl BayerPattern {
    /// Builds a pattern from the four site colours, row-major over the 2x2
    /// tile: `(0,0), (0,1), (1,0), (1,1)` as `(row, col)`.
    ///
    /// # Errors
    ///
    /// [`DecodeError::Unsupported`] unless the tile holds exactly one red,
    /// two green and one blue site.
    pub fn new(sites: [u8; 4]) -> Result<BayerPattern, DecodeError> {
        let count = |c: u8| sites.iter().filter(|&&s| s == c).count();
        if count(RED) != 1 || count(GREEN) != 2 || count(BLUE) != 1 {
            return Err(DecodeError::Unsupported(format!(
                "CFA tile {sites:?} is not an RGGB-family Bayer pattern"
            )));
        }
        Ok(BayerPattern { sites })
    }

    /// Parses a name such as `"RGGB"` or `"GBRG"`.
    ///
    /// # Errors
    ///
    /// [`DecodeError::Unsupported`] for any name that is not four letters
    /// from `R`, `G` and `B` describing a valid Bayer tile.
    pub fn from_name(name: &str) -> Result<BayerPattern, DecodeError> {
        let mut sites = [0u8; 4];
        let letters: Vec<char> = name.chars().collect();
        if letters.len() != 4 {
            return Err(DecodeError::Unsupported(format!(
                "CFA pattern '{name}' is not a 2x2 tile"
            )));
        }
        for (site, letter) in sites.iter_mut().zip(letters) {
            *site = match letter {
                'R' => RED,
                'G' => GREEN,
                'B' => BLUE,
                other => {
                    return Err(DecodeError::Unsupported(format!(
                        "CFA pattern '{name}' has a non-RGB filter '{other}'"
                    )));
                }
            };
        }
        BayerPattern::new(sites)
    }

    /// The colour at pixel `(x, y)` of an image whose origin is the
    /// pattern's `(0, 0)` site.
    #[inline]
    pub fn colour_at(&self, x: usize, y: usize) -> u8 {
        self.sites[(y & 1) * 2 + (x & 1)]
    }

    /// The pattern as seen from an origin moved `dx` columns right and `dy`
    /// rows down, for use after cropping an image at that offset.
    pub fn shifted(&self, dx: usize, dy: usize) -> BayerPattern {
        let mut sites = [0u8; 4];
        for (i, site) in sites.iter_mut().enumerate() {
            *site = self.colour_at(i % 2 + dx, i / 2 + dy);
        }
        BayerPattern { sites }
    }

    /// The `(x, y)` within the 2x2 tile of the first site of `colour`,
    /// scanning row-major. `None` for a colour the pattern lacks.
    pub fn first_site(&self, colour: u8) -> Option<(usize, usize)> {
        self.sites
            .iter()
            .position(|&s| s == colour)
            .map(|i| (i % 2, i / 2))
    }

    /// The four site colours, row-major.
    pub fn sites(&self) -> [u8; 4] {
        self.sites
    }

    /// For each 2x2 parity class and each colour, the `(dx, dy)` offsets of
    /// the 3x3 neighbours that carry that colour (the site itself is
    /// excluded; its own colour has an empty list).
    fn neighbour_table(&self) -> [[Vec<(isize, isize)>; CHANNELS]; 4] {
        let mut table: [[Vec<(isize, isize)>; CHANNELS]; 4] = Default::default();
        for (parity, lists) in table.iter_mut().enumerate() {
            let (px, py) = (parity % 2, parity / 2);
            let own = self.colour_at(px, py);
            for dy in -1isize..=1 {
                for dx in -1isize..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    // Add 2 so the sums stay non-negative; parity is unchanged.
                    let colour = self.colour_at(
                        (px as isize + 2 + dx) as usize,
                        (py as isize + 2 + dy) as usize,
                    );
                    if colour != own {
                        lists[usize::from(colour)].push((dx, dy));
                    }
                }
            }
        }
        table
    }
}

/// Bilinearly demosaics a `width * height` Bayer mosaic into an RGB
/// [`Image`] of the same size.
///
/// `mosaic` is row-major with one sample per pixel; the pixel at `(0, 0)`
/// is the pattern's `(0, 0)` site. Values are copied and averaged without
/// any scaling, so the caller should already have applied the black and
/// white levels. Any size is accepted, including 1xN and 2x2 (see the
/// module documentation for how edges and missing colours behave).
///
/// # Errors
///
/// [`DecodeError::Dimensions`] when `mosaic.len() != width * height`.
pub fn bilinear(
    width: usize,
    height: usize,
    mosaic: &[f32],
    pattern: BayerPattern,
) -> Result<Image, DecodeError> {
    let expected = width.checked_mul(height);
    if expected != Some(mosaic.len()) {
        return Err(DecodeError::Dimensions(negg_types::Error::Dimensions {
            width,
            height,
            len: mosaic.len(),
        }));
    }

    let table = pattern.neighbour_table();
    // Linear offsets for the interior fast path, per parity and colour.
    let w = width as isize;
    let linear: Vec<Vec<Vec<isize>>> = table
        .iter()
        .map(|lists| {
            lists
                .iter()
                .map(|list| list.iter().map(|&(dx, dy)| dy * w + dx).collect())
                .collect()
        })
        .collect();

    let mut out = vec![0.0f32; width * height * CHANNELS];
    for y in 0..height {
        let interior_row = y >= 1 && y + 1 < height;
        for x in 0..width {
            let i = y * width + x;
            let parity = (y & 1) * 2 + (x & 1);
            let own = usize::from(pattern.colour_at(x, y));
            let px = &mut out[i * CHANNELS..i * CHANNELS + CHANNELS];
            px[own] = mosaic[i];
            let interior = interior_row && x >= 1 && x + 1 < width;
            for colour in 0..CHANNELS {
                if colour == own {
                    continue;
                }
                let (sum, count) = if interior {
                    let offsets = &linear[parity][colour];
                    let sum: f32 = offsets
                        .iter()
                        .map(|&off| mosaic[(i as isize + off) as usize])
                        .sum();
                    (sum, offsets.len())
                } else {
                    let mut sum = 0.0;
                    let mut count = 0;
                    for &(dx, dy) in &table[parity][colour] {
                        let nx = x as isize + dx;
                        let ny = y as isize + dy;
                        if nx >= 0 && ny >= 0 && nx < w && ny < height as isize {
                            sum += mosaic[ny as usize * width + nx as usize];
                            count += 1;
                        }
                    }
                    (sum, count)
                };
                px[colour] = if count == 0 { 0.0 } else { sum / count as f32 };
            }
        }
    }

    Ok(Image::from_vec(width, height, out)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RGGB: [u8; 4] = [RED, GREEN, GREEN, BLUE];

    /// Builds a mosaic by sampling `scene(x, y) -> [r, g, b]` through the
    /// pattern.
    fn mosaic_of(
        width: usize,
        height: usize,
        pattern: BayerPattern,
        scene: impl Fn(usize, usize) -> [f32; 3],
    ) -> Vec<f32> {
        let mut m = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                m.push(scene(x, y)[usize::from(pattern.colour_at(x, y))]);
            }
        }
        m
    }

    #[test]
    fn pattern_validation() {
        assert!(BayerPattern::new(RGGB).is_ok());
        assert!(BayerPattern::from_name("GBRG").is_ok());
        assert!(BayerPattern::from_name("RGBE").is_err());
        assert!(BayerPattern::from_name("RRGB").is_err());
        assert!(BayerPattern::from_name("RGGBRGGB").is_err());
        assert!(BayerPattern::new([RED, GREEN, GREEN, 7]).is_err());
    }

    #[test]
    fn colour_at_and_shift() {
        let p = BayerPattern::from_name("RGGB").unwrap();
        assert_eq!(p.colour_at(0, 0), RED);
        assert_eq!(p.colour_at(1, 0), GREEN);
        assert_eq!(p.colour_at(0, 1), GREEN);
        assert_eq!(p.colour_at(1, 1), BLUE);
        assert_eq!(p.colour_at(2, 2), RED);
        assert_eq!(p.shifted(1, 1).sites(), [BLUE, GREEN, GREEN, RED]);
        assert_eq!(p.shifted(1, 0).sites(), [GREEN, RED, BLUE, GREEN]);
        assert_eq!(p.shifted(0, 1).sites(), [GREEN, BLUE, RED, GREEN]);
        assert_eq!(p.shifted(2, 4), p);
        assert_eq!(p.first_site(RED), Some((0, 0)));
        assert_eq!(p.first_site(GREEN), Some((1, 0)));
        assert_eq!(p.first_site(BLUE), Some((1, 1)));
        assert_eq!(p.first_site(9), None);
    }

    #[test]
    fn neighbour_counts_match_bayer_geometry() {
        let p = BayerPattern::from_name("RGGB").unwrap();
        let t = p.neighbour_table();
        // Red site (parity 0): four greens on the edges, four blues on the diagonals.
        assert_eq!(t[0][usize::from(GREEN)].len(), 4);
        assert_eq!(t[0][usize::from(BLUE)].len(), 4);
        assert!(t[0][usize::from(RED)].is_empty());
        // Green site on the red row (parity 1): two reds, two blues.
        assert_eq!(t[1][usize::from(RED)].len(), 2);
        assert_eq!(t[1][usize::from(BLUE)].len(), 2);
        // Blue site (parity 3).
        assert_eq!(t[3][usize::from(GREEN)].len(), 4);
        assert_eq!(t[3][usize::from(RED)].len(), 4);
    }

    #[test]
    fn flat_scene_is_exact_everywhere() {
        let p = BayerPattern::new(RGGB).unwrap();
        let m = mosaic_of(8, 8, p, |_, _| [0.25, 0.5, 0.75]);
        let img = bilinear(8, 8, &m, p).unwrap();
        for y in 0..8 {
            for x in 0..8 {
                assert_eq!(img.pixel(x, y), [0.25, 0.5, 0.75], "at ({x}, {y})");
            }
        }
    }

    #[test]
    fn interior_of_a_linear_ramp_is_exact() {
        // Bilinear interpolation reproduces a plane exactly at interior
        // pixels; integer-valued floats keep the sums exact.
        for name in ["RGGB", "BGGR", "GRBG", "GBRG"] {
            let p = BayerPattern::from_name(name).unwrap();
            let scene = |x: usize, y: usize| {
                [
                    10.0 + x as f32 + 2.0 * y as f32,
                    20.0 + 2.0 * x as f32 + y as f32,
                    30.0 + 3.0 * x as f32 + 3.0 * y as f32,
                ]
            };
            let m = mosaic_of(8, 8, p, scene);
            let img = bilinear(8, 8, &m, p).unwrap();
            for y in 1..7 {
                for x in 1..7 {
                    assert_eq!(img.pixel(x, y), scene(x, y), "{name} at ({x}, {y})");
                }
            }
        }
    }

    #[test]
    fn edges_use_the_available_neighbours() {
        let p = BayerPattern::new(RGGB).unwrap();
        // 3x3 RGGB mosaic:
        //   R G R      1 2 3
        //   G B G  ->  4 5 6
        //   R G R      7 8 9
        let m: Vec<f32> = (1..=9).map(|v| v as f32).collect();
        let img = bilinear(3, 3, &m, p).unwrap();
        // Corner (0,0) is red: green from (1,0) and (0,1), blue from (1,1).
        assert_eq!(img.pixel(0, 0), [1.0, 3.0, 5.0]);
        // Centre (1,1) is blue: red from the four corners, green from the four edges.
        assert_eq!(img.pixel(1, 1), [5.0, 5.0, 5.0]);
        // Edge (1,0) is green: red from (0,0) and (2,0), blue from (1,1).
        assert_eq!(img.pixel(1, 0), [2.0, 2.0, 5.0]);
    }

    #[test]
    fn tiny_images_do_not_panic() {
        let p = BayerPattern::new(RGGB).unwrap();
        let two = bilinear(2, 2, &[1.0, 2.0, 3.0, 4.0], p).unwrap();
        assert_eq!(two.pixel(0, 0), [1.0, 2.5, 4.0]);
        assert_eq!(two.pixel(1, 1), [1.0, 2.5, 4.0]);

        let three = bilinear(3, 3, &[0.5; 9], p).unwrap();
        assert_eq!(three.pixel(2, 2), [0.5, 0.5, 0.5]);

        // 1xN: column 0 alternates R, G; blue never occurs and is 0.
        let m: Vec<f32> = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let tall = bilinear(1, 5, &m, p).unwrap();
        assert_eq!(tall.pixel(0, 0), [1.0, 2.0, 0.0]);
        assert_eq!(tall.pixel(0, 1), [2.0, 2.0, 0.0]);
        assert_eq!(tall.pixel(0, 4), [5.0, 4.0, 0.0]);

        // Nx1 as well, and a 1x1.
        let wide = bilinear(5, 1, &m, p).unwrap();
        assert_eq!(wide.pixel(4, 0), [5.0, 4.0, 0.0]);
        let one = bilinear(1, 1, &[0.7], p).unwrap();
        assert_eq!(one.pixel(0, 0), [0.7, 0.0, 0.0]);

        let empty = bilinear(0, 0, &[], p).unwrap();
        assert_eq!((empty.width(), empty.height()), (0, 0));
    }

    #[test]
    fn length_mismatch_is_a_dimensions_error() {
        let p = BayerPattern::new(RGGB).unwrap();
        match bilinear(2, 2, &[1.0, 2.0, 3.0], p) {
            Err(DecodeError::Dimensions(_)) => {}
            other => panic!("expected Dimensions, got {other:?}"),
        }
    }
}
