# negg-types

Core types for negg: image buffers, settings, roll settings and errors.

## Owns

- `Image<f32>`, the shared linear image buffer type
- `Settings` and `RollSettings`
- The error types used across the engine

## Must not

- Depend on anything beyond serde
- Contain image-processing maths
- Do any file or device I/O

Licence: Mozilla Public License 2.0 (MPL-2.0). See `LICENSE` in this directory.
