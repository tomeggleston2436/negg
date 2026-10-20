# negg-types

Core types for negg: image buffers, settings, roll settings and errors.

## Owns

- `Image`, the shared linear `f32` RGB buffer, with row, band and tile
  iteration helpers (`Tile`, `CHANNELS`)
- `Settings` (with `FilmSettings`, `PrintSettings`, `ColourSettings`,
  `ChannelEstimate`, `OutputSpace`) and `RollSettings`, serde structs that
  load with the plan's defaults and ignore unknown fields
- `ContentHash` and `hash_value`, the deterministic SHA-256 content hash
  that identifies an immutable settings value
- `Error` and `Result`, used across the engine

## Must not

- Depend on anything beyond serde
- Contain image-processing maths
- Do any file or device I/O

Licence: Mozilla Public License 2.0 (MPL-2.0). See `LICENSE` in this directory.
