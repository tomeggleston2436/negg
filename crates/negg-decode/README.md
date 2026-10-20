# negg-decode

RAW and TIFF decoding into linear camera RGB f32 buffers.

## Owns

- `read_tiff`: 8-bit and 16-bit chunky RGB and RGBA TIFF input
  (alpha dropped), normalised to encoded `f32` in `[0, 1]`, with the
  embedded ICC profile bytes when present (`TiffImage`)
- `DecodeError`: I/O, TIFF, unsupported layout and dimension errors
- Reading RAW files via rawler (phase 1)
- Producing linear camera RGB `f32` buffers (phase 1)
- Reporting camera metadata needed downstream (phase 1)

## Must not

- Invert, tone-map or apply any print curve
- Perform colour management or ICC transforms
- Write output files

Licence: Mozilla Public License 2.0 (MPL-2.0). See `LICENSE` in this directory.
