# negg-decode

RAW and TIFF decoding into linear camera RGB f32 buffers.

## Owns

- `decode(path)`: dispatches on extension. `.tif` and `.tiff` (any case)
  go to `decode_tiff`; everything else goes to `decode_raw`. No retry on
  the other path when one fails.
- `decode_raw`: RAW files through rawler (LGPL-2.1, an ordinary crate
  dependency; see `NOTICE`). Applies exactly three things: per-sample
  black and white scaling from the camera's calibrated levels (black to
  0.0, white to 1.0, clipped below at 0, not clipped above), the active
  area crop, and bilinear demosaic of 2x2 Bayer mosaics. No white
  balance, no colour matrix, no transfer curve: the output is camera-native
  linear RGB at unity gain, in sensor orientation. The white level is
  always the camera's calibrated limit (DNG tag or rawler's catalogue),
  never the frame's own maximum, so every frame of a roll decodes on one
  scale. The black level comes from the camera catalogue when rawler has
  one; otherwise it comes from the file's own calibration data (DNG tags,
  or per-frame maker data such as Canon's) or, failing that, from the
  mean of the masked sensor pixels, in which case a warning is recorded.
  Linear (already RGB) DNGs are scaled and cropped without demosaic.
  Monochrome sensors, X-Trans and other non-Bayer CFAs return
  `DecodeError::Unsupported`.
- `decode_tiff`: builds on `read_tiff` and reports how the values are
  encoded through `Transfer`: `Icc(bytes)` when the file embeds a profile,
  `Linear` for 32-bit float samples, otherwise `Gamma22Assumed` with the
  warning "no ICC profile and integer samples: assuming gamma 2.2". The
  values are returned as read; the caller linearises through `negg-cms`.
- `Decoded`: the image, its `Transfer`, rawler's XYZ-to-camera matrix for
  D65 when known (`xyz_to_camera`, recorded for phase 2 and never applied
  here), the camera name, the per-channel white and black levels that were
  mapped to 1.0 and 0.0, and a list of warnings.
- `demosaic`: `BayerPattern` and the pure `bilinear` function, documented
  and tested on synthetic mosaics (flat scenes and linear ramps are exact
  at interior pixels; edges average the neighbours that exist).
- `levels`: the pure `scale_sample` and the repeating `LevelTable` that
  mirrors DNG `BlackLevelRepeatDim` semantics.
- `read_tiff`: 8-bit and 16-bit chunky RGB and RGBA TIFF input (alpha
  dropped) normalised to encoded `f32` in `[0, 1]`, plus 32-bit float RGB
  and RGBA returned as stored, with the embedded ICC profile bytes when
  present (`TiffImage`). 16-bit input is never reduced to 8 bits.
- `DecodeError`: I/O, TIFF, RAW (rawler), unsupported layout and dimension
  errors.

## Tests

Unit tests write a minimal synthetic DNG with the `tiff` crate's encoder
(uncompressed 16-bit CFA with the DNG tags rawler reads) and decode it
through rawler end to end. `tests/corpus.rs` decodes every RAW frame under
`corpus/frames` twice and checks that the data is bit-identical and that
frames from one camera share black and white levels; it passes trivially
when the directory holds no frames.

## Must not

- Invert, tone-map or apply any print curve
- Perform colour management or ICC transforms
- Apply white balance, the camera colour matrix or any sRGB conversion
- Write output files

Licence: Mozilla Public License 2.0 (MPL-2.0). See `LICENSE` in this directory.
