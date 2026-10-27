# 0035: web export

## Status

Accepted, 2026-10-26 (phase 4, task 6).

## Context

Phase 4 adds a Web/Instagram export: a small, ready-to-post JPEG beside the full-size exports. It must look like the full render, never upscale, and be byte-identical whether the app or the CLI produces it.

## Decision

- The web export has a long edge of 2048 px, never upscaled, and 8-bit JPEG quality 85.
- It is always sRGB with the ICC profile embedded, whatever the frame's `output_space`, and is written to `<export dir>/web/<stem>.negg.jpg`. It never overwrites an original.
- The downscale is `negg_render::downscale_to_long_edge`: an area average (box filter with fractional edge weights) of the linear working-space image after geometry and before the sRGB output transform. Scale `t = 2048 / max(w, h)`, output `max(1, round(w t))` by `max(1, round(h t))`, long side exactly 2048.
- Area average was chosen over Lanczos or bicubic because it cannot ring or overshoot (no clipped halos on high-contrast edges such as sprocket holes and scanner holders), is exact for integer ratios, and has no tunable parameters. Accumulation is in `f64` per output pixel in a fixed order, so the result does not depend on thread count.
- No output sharpening, borders, watermarks or reframing.

## Consequences

- Downscaling in linear light is physically correct but makes fine, bright detail slightly softer than a gamma-space resize would; no sharpening compensates for this.
- An area average is somewhat softer than a windowed-sinc filter at non-integer ratios. Accepted for the absence of ringing.
- The constants (2048, 85) are defaults of the web export and are listed in the step 8 table of `docs/MATHS.md`.

## Eval delta

None, export-only: the web export is not part of the corpus eval and no full-size render changes.
