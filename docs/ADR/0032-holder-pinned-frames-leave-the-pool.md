# 0032: frames whose Dmax is pinned at the density cap leave the gamma and cast pools

## Status

Accepted, 2026-10-24. Amends ADR 0017 and ADR 0029 for the gamma and cast candidates; the Dmin pool is unchanged.

## Context

`docs/MATHS.md` step 3 caps density at 6.0, the density of the `1e-6` transmittance floor. A camera scan with the film holder (or another opaque surround) inside the analysed area has its Dmax at that cap: the holder is far denser than any film. Such a frame's range gamma, `Dmax - Dmin`, is about 5 instead of about 1.6. In a real trial in the app most frames of a roll had the holder in shot; they were the majority of the gamma pool, so they were its inliers, the good frames were rejected as outliers, and the pooled gamma of about 5 rendered the whole roll nearly black. Pooling trusted a number that only says "something opaque is in the frame". ADR 0021's analysis area is the fix for the frame; this ADR keeps such frames from damaging the rest of the roll.

## Decision

- **Pinned Dmax.** A frame is pinned when any channel of its Dmax in effect (its fixed Dmax, else its own estimate) is at least `DENSITY_CAP - DMAX_PINNED_TOLERANCE`: `DENSITY_CAP = 6.0` is `-log10(1e-6)`, and `DMAX_PINNED_TOLERANCE = 0.05` (a transmittance within about 1.12 times the floor; Provisional choice, `TODO(measure)`). One function, `negg_types::dmax_pinned`.
- **Out of the gamma and cast pools.** In `pool_roll` a pinned frame is not a gamma candidate and not a cast candidate. Its role there is Unused (Excluded when the user fixed the value, as before). Its pool record carries `needs_analysis_area: true` whatever the pool flags; the field is omitted when false, so existing roll sidecars keep their bytes apart from the version (roll sidecar schema 3, ADR 0021 amendment). The Dmin pool is unaffected: an opaque surround does not move a frame's film base estimate the way it moves Dmax (the border guard already falls back to the percentile).
- **The frame's own render.** A pinned frame renders like any other frame of the roll: with the pooled Dmin, gamma and cast when there are some. Those come from the good frames of the same film and are the best estimate for it; its own pinned range gamma is not. When every frame is pinned there is no pooled gamma (nor cast), never a pool made of pinned values, so each frame keeps its own estimates. The pool does not fix everything for that frame: its exposure anchor and `x_0` (step 5), and its own cast fit when the roll has no cast pool (step 6), are still taken from its analysis grid, holder cells included, so it may still print too light, too dark or tinted. The "needs analysis area" badge is the remedy: an analysis area on the film fixes all of them.
- **Reported.** `negg batch` warns once per pinned frame (`FlowEvent::NeedsAnalysisArea`); `--dump-stats` prints `needs analysis area: Dmax at the density cap` for a pinned frame in every mode; the session's `FrameInfo` and `FrameView` carry `needs_analysis_area` (from the frame's analysis, by the same rule), for the app's "needs analysis area" badge. While a re-pool is pending (`pool_pending`) the flag reflects the frame's last completed analysis.

## Consequences

- A roll of 3 good and 5 held frames pools exactly as the 3 good frames alone, and flags the 5 (`crates/negg-render/src/roll.rs`, `crates/negg-cli/tests/analysis_area.rs`).
- Without a pinned frame nothing changes: every existing output and pool is byte-identical.
- False negatives: a frame whose surround is less opaque, so that its Dmax stays below the tolerance band, is not caught; the analysis area is the general fix. The tolerance is marked `TODO(measure)`.
- False positives: a frame with genuinely very dense film, or one whose darkest samples sit at the decoder's black-level floor (clipped to zero), can reach the band with no holder in shot. It is badged and left out of the gamma and cast pools, but still renders with the pool like any other frame, so the cost is a needless badge and one fewer pool member. Measuring the tolerance on real rolls (`TODO(measure)`) bounds this.
- Cost if wrong: one exclusion rule.

## Eval delta

None: the committed corpus has no frame with a pinned Dmax.
