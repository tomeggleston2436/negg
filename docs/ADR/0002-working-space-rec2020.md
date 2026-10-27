# 0002: working space is linear Rec.2020, D65

## Status

Accepted, 2026-10-20. Decision 2 in `ROADMAP.md`. The working space stands; the way a RAW enters it changed in part in ADR 0026 (the print channels are read as Rec.709 primaries and enter through the BT.2087 matrix, not the camera matrix).

## Context

`docs/MATHS.md` step 7 needs a working space for every post-inversion adjustment and for the GPU preview kernel of phase 4. Everything before step 7 is in camera-native RGB with no defined primaries, so the choice only affects what happens after `out_c` is formed. The candidates were a wide standard space (Rec.2020) and a wider non-standard one (ProPhoto).

## Decision

Use linear Rec.2020 with a D65 white point as the working space. Enter it via the camera matrix recorded in step 1 (camera RGB to XYZ to Rec.2020), Bradford-adapted to D65, applied after inversion. Not ProPhoto.

## Consequences

- `negg-cms` bundles a v4 Rec.2020 ICC profile alongside sRGB, Adobe RGB compatible and Display P3, from a permissive source or generated from primaries.
- Looks (phase 4) are 3D LUTs defined in Rec.2020.
- ProPhoto was considered and ruled out; the plan records the outcome, not the reasoning, so this ADR does not add one.
- Export to ProPhoto is still offered as an output space via `lcms2`; it is only ruled out as the working space.
- Reopening this changes every stored look and sidecar, so it needs a new ADR and a schema version bump.

## Eval delta

None, no render exists yet. The first `negg-cli eval` run of phase 1 is the baseline.
