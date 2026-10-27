// SPDX-License-Identifier: Apache-2.0

//! The binary wire formats a [`crate::Session`] answers with: a preview
//! tile is a 48-byte little-endian header followed by RGBA8 pixels, scopes
//! are a 32-byte header followed by the histogram, parade and vectorscope
//! arrays. Thumbnails are raw JPEG bytes with no header. The desktop app's
//! frontend and the C ABI read these; pixels are never sent as JSON.
//!
//! Only byte layout lives here: the pixels and the scope counts come from
//! `negg-render`.

use negg_render::{SCOPE_BINS, Scopes};

use crate::error::SessionError;

/// Side of a square preview tile in pixels. The value is
/// `negg_render::TILE_SIZE`, the single source, as `u32`.
pub const TILE_SIZE: u32 = {
    assert!(negg_render::TILE_SIZE <= u32::MAX as usize);
    negg_render::TILE_SIZE as u32
};
/// Magic bytes at the start of a tile response.
pub const TILE_MAGIC: [u8; 8] = *b"NEGGTILE";
/// Version of the tile header layout.
pub const TILE_VERSION: u16 = 1;
/// Length of the tile header in bytes; the pixels start here.
pub const TILE_HEADER_LEN: usize = 48;
/// The tile `pixel_format` for RGBA8 sRGB with alpha 255.
pub const TILE_PIXEL_FORMAT_RGBA8: u16 = 1;
/// Magic bytes at the start of a scopes response.
pub const SCOPES_MAGIC: [u8; 8] = *b"NEGGSCOP";
/// Version of the scopes layout.
pub const SCOPES_VERSION: u16 = 1;
/// Length of the scopes header in bytes; the arrays start here.
pub const SCOPES_HEADER_LEN: usize = 32;
/// Total length of a scopes response: the header, the histogram
/// (4 x 256 `u32`), the parade (3 x 256 x 256 `u16`) and the vectorscope
/// (256 x 256 `u16`), 528416 bytes.
pub const SCOPES_LEN: usize = SCOPES_HEADER_LEN
    + 4 * SCOPE_BINS * 4
    + 3 * SCOPE_BINS * SCOPE_BINS * 2
    + SCOPE_BINS * SCOPE_BINS * 2;

/// The fields of a tile header, as [`parse_tile`] reads them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileHeader {
    /// Tile width in pixels, 1 to [`TILE_SIZE`].
    pub width: u32,
    /// Tile height in pixels, 1 to [`TILE_SIZE`].
    pub height: u32,
    /// Column index of the tile in its level.
    pub tile_x: u32,
    /// Row index of the tile in its level.
    pub tile_y: u32,
    /// Pyramid level: 0 is 1:1, `n` is 1/2^n.
    pub level: u32,
    /// Frame index in the roll.
    pub frame: u32,
    /// The session generation the tile was rendered at.
    pub generation: u32,
}

/// A tile response: the 48-byte little-endian header followed
/// by `rgba`, which must hold `width * height * 4` bytes of RGBA8 sRGB,
/// row-major from the top-left with alpha 255.
#[allow(clippy::too_many_arguments)]
pub fn encode_tile(
    frame: u32,
    generation: u32,
    level: u32,
    tile_x: u32,
    tile_y: u32,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Vec<u8> {
    debug_assert_eq!(rgba.len(), width as usize * height as usize * 4);
    let mut out = Vec::with_capacity(TILE_HEADER_LEN + rgba.len());
    out.extend_from_slice(&TILE_MAGIC);
    out.extend_from_slice(&TILE_VERSION.to_le_bytes());
    out.extend_from_slice(&TILE_PIXEL_FORMAT_RGBA8.to_le_bytes());
    for v in [
        width,
        height,
        tile_x,
        tile_y,
        level,
        frame,
        generation,
        TILE_HEADER_LEN as u32,
        0,
    ] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    debug_assert_eq!(out.len(), TILE_HEADER_LEN);
    out.extend_from_slice(rgba);
    out
}

/// Reads a tile response: its header and its RGBA8 pixels.
///
/// # Errors
///
/// [`SessionError::Invalid`] for a short buffer, a wrong magic, a version
/// other than 1, a pixel format other than RGBA8, a byte offset other than
/// 48 or a total length other than `48 + width * height * 4`.
pub fn parse_tile(bytes: &[u8]) -> Result<(TileHeader, &[u8]), SessionError> {
    let bad = |what: &str| SessionError::Invalid(format!("tile: {what}"));
    if bytes.len() < TILE_HEADER_LEN {
        return Err(bad("shorter than its header"));
    }
    if bytes[..8] != TILE_MAGIC {
        return Err(bad("wrong magic"));
    }
    if u16_at(bytes, 8) != TILE_VERSION {
        return Err(bad("unknown version"));
    }
    if u16_at(bytes, 10) != TILE_PIXEL_FORMAT_RGBA8 {
        return Err(bad("unknown pixel format"));
    }
    if u32_at(bytes, 40) as usize != TILE_HEADER_LEN {
        return Err(bad("byte offset is not 48"));
    }
    let header = TileHeader {
        width: u32_at(bytes, 12),
        height: u32_at(bytes, 16),
        tile_x: u32_at(bytes, 20),
        tile_y: u32_at(bytes, 24),
        level: u32_at(bytes, 28),
        frame: u32_at(bytes, 32),
        generation: u32_at(bytes, 36),
    };
    let expected = (header.width as usize)
        .checked_mul(header.height as usize)
        .and_then(|n| n.checked_mul(4))
        .and_then(|n| n.checked_add(TILE_HEADER_LEN));
    if expected != Some(bytes.len()) {
        return Err(bad("length does not match its size"));
    }
    Ok((header, &bytes[TILE_HEADER_LEN..]))
}

/// A scopes response: the 32-byte little-endian header (magic,
/// version, reserved, frame, generation, byte offset 32, measured width and
/// height), then the histogram as 4 x 256 `u32` (R, G, B, luma), the
/// parade as 3 x 256 x 256 `u16` (`[c][value][col]`) and the vectorscope as
/// 256 x 256 `u16` (`[row][col]`), all little-endian; [`SCOPES_LEN`] bytes.
pub fn encode_scopes(frame: u32, generation: u32, s: &Scopes) -> Vec<u8> {
    let mut out = Vec::with_capacity(SCOPES_LEN);
    out.extend_from_slice(&SCOPES_MAGIC);
    out.extend_from_slice(&SCOPES_VERSION.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    for v in [
        frame,
        generation,
        SCOPES_HEADER_LEN as u32,
        s.width,
        s.height,
    ] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    debug_assert_eq!(out.len(), SCOPES_HEADER_LEN);
    for channel in &s.histogram {
        for v in channel {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    for v in s.parade.iter().chain(&s.vectorscope) {
        out.extend_from_slice(&v.to_le_bytes());
    }
    debug_assert_eq!(out.len(), SCOPES_LEN);
    out
}

/// The little-endian `u16` at `at`.
fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

/// The little-endian `u32` at `at`.
fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_header_bytes_for_a_known_tile() {
        let rgba: Vec<u8> = (0..2 * 3 * 4).map(|i| i as u8).collect();
        let bytes = encode_tile(7, 42, 1, 3, 2, 2, 3, &rgba);
        let mut expected = Vec::new();
        expected.extend_from_slice(b"NEGGTILE");
        expected.extend_from_slice(&[1, 0, 1, 0]);
        for v in [2u32, 3, 3, 2, 1, 7, 42, 48, 0] {
            expected.extend_from_slice(&v.to_le_bytes());
        }
        assert_eq!(&bytes[..TILE_HEADER_LEN], &expected[..]);
        assert_eq!(&bytes[TILE_HEADER_LEN..], &rgba[..]);
        let (header, pixels) = parse_tile(&bytes).unwrap();
        assert_eq!(
            header,
            TileHeader {
                width: 2,
                height: 3,
                tile_x: 3,
                tile_y: 2,
                level: 1,
                frame: 7,
                generation: 42,
            }
        );
        assert_eq!(pixels, &rgba[..]);
    }

    #[test]
    fn a_bad_tile_is_rejected() {
        let good = encode_tile(0, 0, 0, 0, 0, 1, 1, &[1, 2, 3, 255]);
        assert!(parse_tile(&good[..40]).is_err());
        assert!(parse_tile(&good[..good.len() - 1]).is_err());
        for (at, v) in [(0usize, b'X'), (8, 2), (10, 2), (40, 47)] {
            let mut bad = good.clone();
            bad[at] = v;
            assert!(parse_tile(&bad).is_err(), "byte {at}");
        }
    }

    #[test]
    fn scopes_layout_and_length() {
        let mut s = Scopes {
            width: 640,
            height: 480,
            histogram: [[0; SCOPE_BINS]; 4],
            parade: vec![0; 3 * SCOPE_BINS * SCOPE_BINS],
            vectorscope: vec![0; SCOPE_BINS * SCOPE_BINS],
        };
        s.histogram[3][255] = 0x0102_0304;
        s.parade[0] = 0x0506;
        s.vectorscope[SCOPE_BINS * SCOPE_BINS - 1] = 0x0708;
        let bytes = encode_scopes(3, 9, &s);
        assert_eq!(bytes.len(), 528_416);
        assert_eq!(SCOPES_LEN, 528_416);
        assert_eq!(&bytes[..8], b"NEGGSCOP");
        assert_eq!(&bytes[8..12], &[1, 0, 0, 0]);
        assert_eq!(u32_at(&bytes, 12), 3);
        assert_eq!(u32_at(&bytes, 16), 9);
        assert_eq!(u32_at(&bytes, 20), 32);
        assert_eq!(u32_at(&bytes, 24), 640);
        assert_eq!(u32_at(&bytes, 28), 480);
        let luma_last = SCOPES_HEADER_LEN + (3 * SCOPE_BINS + 255) * 4;
        assert_eq!(u32_at(&bytes, luma_last), 0x0102_0304);
        let parade = SCOPES_HEADER_LEN + 4 * SCOPE_BINS * 4;
        assert_eq!(u16_at(&bytes, parade), 0x0506);
        assert_eq!(u16_at(&bytes, SCOPES_LEN - 2), 0x0708);
    }
}
