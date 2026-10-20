# negg-decode

RAW and TIFF decoding into linear camera RGB f32 buffers.

## Owns

- Reading RAW files (via rawler) and TIFF input
- Producing linear camera RGB `f32` buffers
- Reporting camera metadata needed downstream

## Must not

- Invert, tone-map or apply any print curve
- Perform colour management or ICC transforms
- Write output files

Licence: Mozilla Public License 2.0 (MPL-2.0). See `LICENSE` in this directory.
