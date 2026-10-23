// SPDX-License-Identifier: MPL-2.0

//! Black and white level scaling.
//!
//! A sensor's calibrated black level maps to 0.0 and its calibrated white
//! level to 1.0. The levels come from the camera's documented limits (the
//! DNG tags or rawler's camera catalogue), never from the frame's own
//! minimum or maximum, so every frame of a roll lands on the same scale.
//! Values below black are clipped to 0; values above white are kept, so a
//! sensor that records past its nominal white level yields values above 1.

use crate::error::DecodeError;

/// Scales one sample so that `black` maps to 0.0 and `white` to 1.0.
///
/// The result is clipped below at 0.0 and not clipped above. The caller
/// must have checked with [`check_levels`] that `white > black`.
#[inline]
pub fn scale_sample(value: f32, black: f32, white: f32) -> f32 {
    ((value - black) / (white - black)).max(0.0)
}

/// Checks that a black and white level pair is usable for scaling.
///
/// # Errors
///
/// [`DecodeError::Unsupported`] when `white <= black` or either level is
/// not a finite number.
pub fn check_levels(black: f32, white: f32) -> Result<(), DecodeError> {
    if !black.is_finite() || !white.is_finite() || white <= black {
        return Err(DecodeError::Unsupported(format!(
            "white level {white} is not above black level {black}"
        )));
    }
    Ok(())
}

/// A table of per-sample levels that repeats across the image, in the DNG
/// `BlackLevelRepeatDim` sense: `values` holds `width * height * planes`
/// entries, row-major over the repeat tile, with `planes` consecutive
/// entries per tile position.
#[derive(Clone, Debug, PartialEq)]
pub struct LevelTable {
    values: Vec<f32>,
    width: usize,
    height: usize,
    planes: usize,
}

impl LevelTable {
    /// Builds a table. A single value makes a 1x1x1 table that applies to
    /// every sample.
    ///
    /// # Errors
    ///
    /// [`DecodeError::Unsupported`] when any dimension is zero or the value
    /// count does not match `width * height * planes`.
    pub fn new(
        values: Vec<f32>,
        width: usize,
        height: usize,
        planes: usize,
    ) -> Result<LevelTable, DecodeError> {
        if width == 0 || height == 0 || planes == 0 || values.len() != width * height * planes {
            return Err(DecodeError::Unsupported(format!(
                "level table has {} entries for a {width}x{height} repeat with {planes} planes",
                values.len()
            )));
        }
        Ok(LevelTable {
            values,
            width,
            height,
            planes,
        })
    }

    /// A table holding one value for every sample.
    pub fn uniform(value: f32) -> LevelTable {
        LevelTable {
            values: vec![value],
            width: 1,
            height: 1,
            planes: 1,
        }
    }

    /// The level for the sample at `(row, col)` in plane `plane`. A table
    /// with fewer planes than asked for shares its planes cyclically, so a
    /// one-plane table applies to every plane.
    #[inline]
    pub fn at(&self, row: usize, col: usize, plane: usize) -> f32 {
        let tile = (row % self.height) * self.width + (col % self.width);
        self.values[tile * self.planes + plane % self.planes]
    }

    /// Width of the repeat tile.
    pub fn width(&self) -> usize {
        self.width
    }

    /// Height of the repeat tile.
    pub fn height(&self) -> usize {
        self.height
    }

    /// Planes per tile position.
    pub fn planes(&self) -> usize {
        self.planes
    }
}

/// Checks every black and white pairing the two tables produce over one
/// combined repeat tile.
///
/// # Errors
///
/// [`DecodeError::Unsupported`] from [`check_levels`] on the first bad pair.
pub fn check_tables(black: &LevelTable, white: &LevelTable) -> Result<(), DecodeError> {
    let rows = black.height().max(white.height());
    let cols = black.width().max(white.width());
    let planes = black.planes().max(white.planes());
    for row in 0..rows {
        for col in 0..cols {
            for plane in 0..planes {
                check_levels(black.at(row, col, plane), white.at(row, col, plane))?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_between_black_and_white() {
        let (black, white) = (512.0, 16383.0);
        assert_eq!(scale_sample(512.0, black, white), 0.0);
        assert_eq!(scale_sample(16383.0, black, white), 1.0);
        assert_eq!(scale_sample(300.0, black, white), 0.0);
        assert!(scale_sample(20000.0, black, white) > 1.0);
        let mid = scale_sample(8447.5, black, white);
        assert!((mid - 0.5).abs() < 1e-6, "{mid}");
    }

    #[test]
    fn rejects_bad_levels() {
        assert!(check_levels(0.0, 1.0).is_ok());
        assert!(check_levels(512.0, 512.0).is_err());
        assert!(check_levels(600.0, 512.0).is_err());
        assert!(check_levels(f32::NAN, 1.0).is_err());
        assert!(check_levels(0.0, f32::INFINITY).is_err());
    }

    #[test]
    fn table_repeats_and_shares_planes() {
        let t = LevelTable::new(vec![1.0, 2.0, 3.0, 4.0], 2, 2, 1).unwrap();
        assert_eq!(t.at(0, 0, 0), 1.0);
        assert_eq!(t.at(0, 1, 0), 2.0);
        assert_eq!(t.at(1, 0, 0), 3.0);
        assert_eq!(t.at(1, 1, 0), 4.0);
        assert_eq!(t.at(2, 3, 0), 2.0);
        assert_eq!(t.at(3, 2, 2), 3.0);

        let u = LevelTable::uniform(7.0);
        assert_eq!(u.at(5, 9, 2), 7.0);

        let p = LevelTable::new(vec![1.0, 2.0, 3.0], 1, 1, 3).unwrap();
        assert_eq!(p.at(4, 4, 2), 3.0);
    }

    #[test]
    fn table_rejects_bad_shapes() {
        assert!(LevelTable::new(vec![1.0, 2.0], 2, 2, 1).is_err());
        assert!(LevelTable::new(vec![], 0, 1, 1).is_err());
    }

    #[test]
    fn check_tables_finds_a_bad_position() {
        let black = LevelTable::new(vec![10.0, 20.0, 30.0, 4000.0], 2, 2, 1).unwrap();
        let white = LevelTable::uniform(1000.0);
        assert!(check_tables(&black, &white).is_err());
        let good = LevelTable::uniform(0.0);
        assert!(check_tables(&good, &white).is_ok());
    }
}
