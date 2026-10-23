# 0014: rig correction order, flat field and crosstalk

## Status

Proposed, 2026-10-23 (controller rulings 35 to 38 pending owner review).

## Context

`docs/MATHS.md` step 2 (phase 2, `negg-calib`) lists three optional rig corrections: flat field, bare-light crosstalk unmix and the camera-to-density fit. The plan fixes the formulas but not how the calibration frames are decoded, in what order the corrections run relative to the analysis grid, how the flat field blur is computed at a practical cost, or which statistic builds the crosstalk matrix. A full-resolution Gaussian at sigma about 94 px on a 24 MP frame costs about 40 G operations per frame, which would break gate 1 timing.

## Decision

- Ruling 35 (R-rig-decode): decode flat, band and wedge frames with `negg_decode::decode`. They must have `Transfer::Linear` (RAW or float TIFF); anything else is an error. Step 1 scaling is the same as for frames; no flat field or unmix is applied to the flat itself. A rig profile is only valid for the same linear scale as the frames it corrects.
- Ruling 36 (R-rig-order): step 2 runs on the step 1 output, before the analysis grid: flat field, then unmix (clip at 0), then the density model replaces the density formula of step 3 (Ruling 40). Each part is optional. The caller applies it with `negg_render::apply_rig_linear` on its owned image, so `render_frame*` still takes `&Image`.
- Ruling 37 (R-flat): reduce the flat `F` to the step 3 analysis grid (`negg_film::analysis_grid(F, 1024)`, block medians) and blur it there with a separable Gaussian of `sigma_cells = (long_side_px / 64) / block`, kernel radius `ceil(3 * sigma_cells)`, weights normalised to sum 1 and symmetric reflection at the edges (index -1 maps to 0, -2 to 1, repeated for kernels wider than the grid). `mean(F_c)` is the mean over grid cells of the unblurred block medians. The gain cell is `mean_c / blur_c` clamped to [0.25, 4] (a zero blur gives 4). Apply per pixel by bilinear interpolation between cell centres in normalised coordinates, `u = (x + 0.5) / W * gw - 0.5` clamped to [0, gw - 1], and likewise for `v`. The flat must have the frame's exact decoded width and height, else error. Reject the flat as clipped when more than 1% of grid cells have any channel >= 0.98.
- Ruling 38 (R-crosstalk): for band frame `j` (R, G, B lit alone), the response `r_j` is the per-channel `patch_median` of its flat-corrected grid over the central rect {x 0.25, y 0.25, w 0.5, h 0.5}. Column `j` of `M` is `r_j / r_j[j]`. Error when `r_j[j]` is not the largest channel of `r_j` (frames swapped or not narrow band) or `|det M| < 1e-6`. The profile stores `M`; the runtime stores `M^-1` and applies `I <- max(M^-1 I, 0)`.

## Consequences

- The blur runs on about 1024 cells on the long side, so its cost is negligible. The block median adds under 4 px of extra smoothing against a 94 px sigma, which is why the grid is acceptable for the plan's "blurred with sigma = long side / 64".
- The 1% clipped-cell fraction, the 0.98 clip level, the 1e-6 determinant floor and the central rect are Controller choices, `TODO(measure)` (listed in the MATHS.md constants table).
- Flat and band frames must be linear. A flat saved as a gamma-encoded TIFF is refused rather than guessed at.
- `render_frame*` keeps its signature and phase 1 output; the rig correction is an explicit step the caller owns.

Cost if wrong: Ruling 35, accepting ICC TIFF flats later; Ruling 36, reorder two calls; Ruling 37, the blur moves to full resolution and is slower; Ruling 38, a statistic swap.

## Eval delta

None, no render change without a rig profile. The corpus has no rig frames yet.
