# 0018: settings and sidecar schema version 2, profile paths, cast source

## Status

Proposed, 2026-10-23.

## Context

Phase 2 adds a fixed cast to `ColourSettings`, profile references to `Settings`, per-frame overrides and a roll marker to the sidecar, and a neutral offset and cast source to `Estimates`. Older sidecars exist (phase 1). `docs/MATHS.md` step 8 says the sidecar carries a `schema_version` and the source of each estimate. A profile reference needs a path rule that survives moving a roll directory.

## Decision

- Schema: `SETTINGS_SCHEMA_VERSION` goes 1 to 2 (adds `colour.cast` and `profiles`); `SIDECAR_SCHEMA_VERSION` goes 1 to 2 (adds `overrides` and `roll`; `Estimates` gains `neutral_offset` and `cast_source`). `migrate` v1 to v2 edits the JSON value: `schema_version = 2`; `settings.schema_version = 2`; when `estimates` is an object, insert `neutral_offset = [0,0,0]` and `cast_source = "fit"` if `cast.cells_used > 0` else `"identity"`. `parse_settings` sets `schema_version` to the current value after loading a v1 Settings file. The roll sidecar starts at its own version 1. The pinned default hash in `settings.rs` is recomputed.
- Paths: `Settings.profiles` paths resolve relative to the directory of the file that holds them (the roll or frame sidecar, which share a directory); CLI flag paths are made absolute before storing. In `negg convert` without a roll sidecar, relative `profiles` paths in a `--settings` file resolve against the frame's directory, which is where its sidecar will be, not against the settings file's directory.
- Cast source: `CastSource` is `{ Fit, Fixed, Roll, Identity }`. It is `Identity` when auto colour is off, fewer than 8 cells survive, or a film correction is in use.

## Consequences

- A v1 sidecar migrates losslessly: zero offset reproduces phase 1 rendering, and `cast_source` is derived from the stored cell count. A version newer than the build understands (3 or later today) is rejected with an error asking the user to upgrade; `migrate` is the only reader of frame sidecars, and the roll sidecar rejects any version other than its own 1.
- The default settings hash changes, so any recorded hash of default settings changes with it; the migration does not touch hashes of existing files.
- A roll directory can be moved or copied together with its relative profile paths; absolute flag paths stay where they were stated.
- `cast_source` lets the eval and the user see why no cast was applied.

Cost if wrong: schema, one migration step; paths, the path rule; cast source, an enum variant.

## Eval delta

None, no render change. Phase 1 sidecars migrate to the same renders.
