# 0033: saturation about the Rec.2020 luminance

## Status

Accepted, 2026-10-26 (phase 4, task 1).

## Context

Phase 4 adds tone presets that set the grade, toe, shoulder, black point compensation and a saturation (`docs/MATHS.md` steps 5 and 7). The print stage alone cannot change colourfulness without moving the cast and neutral behaviour, and step 7 had no saturation control. A preset also needs a label to show which preset the numbers came from.

## Decision

- Add `print.saturation` (default 1, valid 0 to 2, finite) and `print.preset` (an optional label that never affects the render) to `PrintSettings`, with the matching optional `saturation` and `preset` in `PrintOverrides`. Both are omitted from files at their defaults, so a default settings value serialises exactly as schema 4 apart from `schema_version`.
- Apply saturation in the fused kernel after the entry matrix (or after the print stage when there is no matrix) and before the output transform: `y = 0.2627 r + 0.6780 g + 0.0593 b`, `out_c = max(0, y + s * (c - y))`, skipped when `s` is exactly 1 (`negg_render::saturate_pixel`).
- Store saturation in `PrintSettings` because `PrintCoefficients` is already built from it and already reaches `render_in_place`, so no new argument travels through `render_full`, `render_region` or the pipeline.
- Bump the settings schema to 5, the frame sidecar to 5 and the roll sidecar to 4. The ladders only set the version numbers: absent means saturation 1 and no preset. Files newer than the build are still refused.

## Consequences

- Saturation preserves neutrals and the Rec.2020 luminance for every value; values above 1 can clip a saturated channel at 0 (out of gamut), which the output transform then handles as for any other out-of-gamut pixel.
- Untagged sources have no Rec.2020 working space, so the same weights act on the scan's own RGB. This is a small inexactness accepted for a creative control.
- Cost: one more field in the settings hash domain only when it is not 1.

## Eval delta

None at the default: saturation 1 skips the step and the golden render is byte-identical.
