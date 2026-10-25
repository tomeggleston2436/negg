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
  defaults. Version 3 adds `geometry`, and version 4 adds
  `geometry.analysis_area`; older values load with the defaults
- `geometry` module (phase 3): `Geometry` (the crop, the optional analysis
  area, `analysis_buffer`, quarter-turn rotation and flip), `CropRect` and
  the pixel rects the analysis and the export use. `Geometry::apply` only
  moves pixels, so it lives here rather than in `negg-render`
- `Estimates` (with `DminSource`, `GammaSource`, `CastSource` and
  `CastCorrection`), the per-frame analysis results written into the sidecar;
  `neutral_offset` and `cast_source` are new in phase 2; `DENSITY_CAP`,
  `DMAX_PINNED_TOLERANCE` and `dmax_pinned`, which say whether a Dmax sits at
  the density cap (ADR 0032)
- `rig` module: the runtime corrections `RigCorrection`, `FlatField`,
  `DensityModel` and `FilmCorrection`, and `PatchRect`
- `profile` module: the serde forms `RigProfile` and `FilmProfile` of the
  `*.negg.toml` profile files, with `validate`, `RigProfile::density_model`
  and `FilmProfile::correction` (TOML I/O lives in `negg-io`)
- `roll` module: `FrameOverrides` and `resolve` (roll settings plus a
  frame's overrides), and the roll pool record `RollPool`, `PoolFrame`
  (with `needs_analysis_area`), `PoolRole` and `PooledEstimates`
- `ContentHash` and `hash_value`, the deterministic SHA-256 content hash
  that identifies an immutable settings value
- `Error` and `Result`, used across the engine

## Must not

- Depend on anything beyond serde
- Contain image-processing maths
- Do any file or device I/O

Licence: Mozilla Public License 2.0 (MPL-2.0). See `LICENSE` in this directory.
