# 0027: step 6 cast fit classifies cells after a provisional correction

## Status

Accepted, 2026-10-24. Supersedes the parts of ADR 0009 that choose the fitted cells (hue and luma read on the uncorrected cell, the per-band selection as the only stage) and its slope clamp [0.8, 1.2]. The definitions of luma, chroma and hue on `-e`, the fitted lines and the applied correction of ADR 0009 as amended stand.

## Context

`docs/MATHS.md` step 6 (ADR 0009 as amended) chose the fitted cells by reading luma, chroma and hue on the uncorrected `-e`, and clamped the fitted slopes to [0.8, 1.2]. The first real camera scans showed three weaknesses, independent of the step 7 matrix defect fixed by ADR 0026:

1. A cast moves neutral cells in hue. A pure blue cast (hue about 240) or green cast (hue about 120) puts a grey scene in the sky or foliage sector, so it is excluded and the fit is the identity: step 6 could remove red, yellow and magenta casts but never blue or green ones. On one frame 93 to 97% of cells landed in the sky sector.
2. The clamp of plus or minus 0.2 was far smaller than the correction a real cast needed.
3. Grey world pulls scenes dominated by one warm hue: when a luma band is almost all one saturated colour, its lowest-chroma 30% is that colour, and the fit turns it grey and pushes the frame the other way. On a dark frame the bands carried almost no information.

## Decision

Choose the fitted cells in two stages and keep the model and the applied correction as they were (`e_R = s_R e_G + o_R`, `e_B = s_B e_G + o_B`, fitted on the uncorrected `e`; `e_c <- (e_c - o_c) / s_c`):

1. **Provisional selection:** the lowest-chroma 30% of all cells, read on the uncorrected cells, with no luma bands and no hue sectors. Fit. Fewer than 8 cells gives the identity, as before.
2. **Refinement**, at most 4 times: correct every cell with the current fit; read luma, chroma and hue on the corrected cell; split into the 20/80 luma bands of the corrected luma; drop cells whose corrected hue is in the foliage (90 to 160) or sky (200 to 260) sector and cells whose corrected chroma exceeds 0.1; keep the lowest-chroma 30% of each band; pool and refit. Stop when the selection does not change. If fewer than 8 cells survive, keep the previous fit.

Every fit is ordinary least squares with the slopes bounded to [0.5, 2]. The returned fit reports a slope only when its own cells identify one: when their `e_G` has a standard deviation below 0.03, its slopes are set to 1. The provisional fit and the refits in between are not held to that rule. After the bound or the rule the offset goes through the centroid of the selected cells, `o = mean(e_target) - s * mean(e_G)`, as before.

## Consequences

- **Blue and green casts (1).** After the provisional correction a grey scene under any cast reads as neutral, so the sectors no longer remove it. Sky and foliage beside neutrals are still excluded in the expected case: the provisional fit is pulled part of the way towards them, and when that pull is roughly along their own hue they stay in their sector and the refinement drops them. This is not a guarantee: a pull at an angle to a cell's hue can move a cell near a sector edge out of it (the provisional correction removes a pull of hue about 250, which can push a sky cell near the 200 degree edge out of the sky sector, where it can turn cyan), and such a cell then takes part in the refit. The test that covers this has weak sky (hue 220) and foliage (hue 140) under a magenta cast with greys as a 10% minority, so the 30% cut must take coloured cells unless the sectors remove them: with the sectors the greys end within 1e-4 of neutral, and without them up to 1.7e-3 off.
- **One-colour frames.** A frame that is one colour everywhere cannot be told apart from a grey scene under a cast of that colour, and requirement 1 asks for such casts to be removed, so it is now corrected to neutral (before, all foliage or all sky gave the identity). A frame of grass of varying saturation with no neutral in it is pulled partly grey. Roll pooling is the mitigation.
- **Slope bound (2).** [0.5, 2] is the validation range of a Fixed cast in `negg-types`, so every fitted or pooled cast is a valid Fixed cast and `negg lock` can copy it. The bound keeps the correction monotone and finite. The tight clamp used to stand in for "the slope is not identified"; that is now the explicit spread rule on the returned fit.
- **Spread rule, and why not on the provisional fit.** Under a slope cast the lowest-chroma cells cluster where the fitted lines cross neutral, so the provisional selection can be narrow in `e_G` even on a grey scene with a wide range. Held to the spread rule, the provisional slope stayed at 1, the chroma rejection then dropped the cells far from the crossing, and the fit locked at slope 1 (review of this ADR: a 1.6 slope on `e_G` in [-0.7, -0.2] left 0.23 of residual). Fitting the provisional slope freely lets the first refinement see a neutral scene and widen to every band. A selection that is still narrow after the refinements carries no slope information, so the returned fit falls back to slope 1 and an offset.
- **Dark frames (3).** The spread rule is the mitigation for a dark landscape whose bands hold almost only near-black cells near `e = 0`: those cells span too little of `e_G` to identify a slope, so the frame gets an offset correction and no extrapolated slope.
- **Warm-dominated scenes (3).** The global provisional stage picks the neutrals whenever they are less saturated than the scene colour, and the corrected chroma rejection makes a band of saturated rock contribute nothing instead of its least saturated rock. A cast larger than the scene colour and opposite to it can still win on a single frame; no single-frame statistic separates the two.
- **Roll mode.** `pool_roll` and `pool_cast` are unchanged: each parameter is the weighted median of its own values. The weight is still `cells_used`, now the size of the selection the returned fit was made on (the last refinement); it is 0 only when the provisional selection is too small.
- **Cost.** The refinement re-reads the grid up to 4 times; a 24 MP render went from 751 ms to 803 ms (gate 2000 ms).
- **Golden re-baseline.** `crates/negg-cli/tests/data/golden.phase1.negg.tif` was the phase 1 binary's output. On its neutral ramp both the old and the new fit are the identity within about 2e-5 in slope, but from different cells, which moves the exposure anchor by about 1e-6 and rounds 468 of 3072 pixels by one 16-bit code value in each channel. The file was regenerated from this change (`negg convert --no-roll` on the test's deterministic input) and the test renamed `no_roll_matches_the_golden_render_byte_for_byte`: it now guards the exact bytes of the default `--no-roll` render. Rule: a deliberate engine change re-baselines it with an ADR.

Changed defaults and constants:

| Constant | Was | Now | Why |
| --- | --- | --- | --- |
| slope bound | [0.8, 1.2] | [0.5, 2] | The Fixed cast validation range. |
| cells for hue, luma and chroma | uncorrected `-e` | `-e` after the current fit | Fixes 1. |
| provisional selection | none | lowest-chroma 30% of all cells | Reuses the 30% of ADR 0009. |
| neutral chroma max | none | 0.1 | Provisional choice, `TODO(measure)`. |
| minimum `e_G` spread for a slope | any spread (`sxx > 0`) | standard deviation 0.03 over the cells of the returned fit, else slopes 1 | Provisional choice, `TODO(measure)`. A neutral range of 0.15 of the normalised span (standard deviation about 0.043) identifies a slope; cells within about 0.05 of one level do not. |
| refinements | none | at most 4 | Provisional choice, `TODO(measure)`. |
| minimum cells | 8 | 8 | Unchanged; below it the provisional fit gives the identity and a refinement keeps the previous fit. |

Tests in `crates/negg-render/src/cast.rs` that failed before this change: a grey scene under a pure blue cast and under a pure green cast are corrected to neutral within 1e-3; slopes 1.6 and 0.65 are recovered within 1e-3 on `e_G` over [-1, 0] and, after review, on `e_G` over [-0.7, -0.2] and [-0.6, -0.3]; a scene of 60% saturated golden rock at mid luma and 40% greys under a known cast ends with the greys neutral within 1e-3 and the rock's hue within 1 degree and chroma within 1e-3.

## Eval delta

None measured: the committed corpus has no frames. The golden render changes by at most one code value (above).
