# negg-types

Core types for negg: image buffers, settings, roll settings and errors.

## Owns

- `Image`, the shared linear `f32` RGB buffer, with row, band and tile
  iteration helpers (`Tile`, `CHANNELS`)
- `Settings` (with `FilmSettings`, `PrintSettings`, `ColourSettings`,
  `ChannelEstimate`, `OutputSpace`) and `RollSettings`, serde structs that
  load with the plan's defaults and ignore unknown fields
  Schema version 2 adds `colour.cast` (`CastEstimate`) and `profiles`
  (`ProfileRefs`); a version 1 settings value still loads with both at their
  defaults
- `Estimates` (with `DminSource`, `GammaSource`, `CastSource` and
  `CastCorrection`), the per-frame analysis results written into the sidecar;
  `neutral_offset` and `cast_source` are new in phase 2
- `rig` module: the runtime corrections `RigCorrection`, `FlatField`,
  `DensityModel` and `FilmCorrection`, and `PatchRect`
- `profile` module: the serde forms `RigProfile` and `FilmProfile` of the
  `*.negg.toml` profile files, with `validate`, `RigProfile::density_model`
  and `FilmProfile::correction` (TOML I/O lives in `negg-io`)
- `roll` module: `FrameOverrides` and `resolve` (roll settings plus a
  frame's overrides), and the roll pool record `RollPool`, `PoolFrame`,
  `PoolRole` and `PooledEstimates`
- `ContentHash` and `hash_value`, the deterministic SHA-256 content hash
  that identifies an immutable settings value
- `Error` and `Result`, used across the engine

## Must not

- Depend on anything beyond serde
- Contain image-processing maths
- Do any file or device I/O

Licence: Mozilla Public License 2.0 (MPL-2.0). See `LICENSE` in this directory.
