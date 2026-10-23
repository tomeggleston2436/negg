# 0008: the exposure anchor and how `x_0` is solved

## Status

Proposed, 2026-10-20 (controller ruling pending owner review).

## Context

`docs/MATHS.md` step 5 says `x_0` is chosen so that the frame's exposure anchor prints at `P = 0.75`, and that the anchor is "the trimmed mean of textured grid cells in the midtones". That is all the plan says. `docs/MATHS.md` carried the line "TODO(spec): the plan does not say how 'textured' cells are selected, what density band counts as 'midtones', what fraction the trimmed mean trims, or whether the anchor is one value or one per channel. The sign conventions and display units of `x_0` and `cmy_c` as user-facing sliders are also open." `exposure_anchor` in `negg-film` and `solve_x0` in `negg-render` need all of this, so this ADR records the ruling (contract item R-anchor).

## Decision

The anchor is one scalar `a`, computed on the analysis grid after step 4 and after the step 6 cast correction has been applied to the grid (the anchor is taken on the cast-corrected grid, as `render_frame` in `negg-render` does) as follows. Luma is `l = (e_R + e_G + e_B) / 3` per cell. A cell is textured when the coefficient of variation (std/mean) of its block's linear camera luma is at least 0.03, the complement of the flat threshold of step 3. Midtone cells are the textured cells whose `l` lies between the 25th and 75th percentiles of `l` over all textured cells. `a` is the 10% trimmed mean of `l` over the midtone cells (drop the lowest 10% and the highest 10%, then take the mean); if fewer than 16 midtone cells exist, `a` is the median of `l` over all grid cells instead. Solve `x_0` analytically so that `x = a * k + x_0` (sign per ADR 0011) prints at `P = 0.75`, with `softplus_inv(y) = ln(e^y - 1)`:

```
v        = Dmax_p - softplus_inv(a_sh * (Dmax_p - 0.75)) / a_sh
x_target = Dmin_p + softplus_inv(a_hl * (v - Dmin_p)) / a_hl
x_0      = x_target - a * k
```

The user's `Settings.print.exposure` is added to `x_0`; positive means more print density, that is a darker print. `cmy_c` is added to `x_c` per channel; positive means more print density in that channel.

## Consequences

- The anchor is a luma scalar, so the three channels share one `x_0`. Channel balance is the job of step 6 and of `cmy_c`, not of exposure; a per-channel anchor would have made the exposure solve a second, competing cast correction.
- The solve is exact and closed-form because both softplus terms are invertible for the arguments involved: `softplus_inv` is only called on positive values (`Dmax_p - 0.75` and `v - Dmin_p` are positive with any sensible paper constants). With the step 5 defaults (`Dmin_p = 0.06`, `Dmax_p = 2.3`, `a_hl = 3.0`, `a_sh = 6.0`) the solve gives `v = 0.75002` and `x_target = 0.70505`; running `x_target` forward through the two softplus equations returns `P = 0.75` to within floating-point rounding, which is the unit test for `solve_x0`.
- `x_0` depends on `a` and on `k`, so changing the grade `R` re-solves `x_0` and the anchor stays at `P = 0.75`: the grade pivots around the anchor rather than around paper white. The user's `exposure` is added after the solve and is not part of it (`solve_x0` takes the anchor and the print settings only).
- Both user controls are in log print density, the same unit as `x_0` and `cmy_c` in the step 5 equations. No display unit conversion exists in phase 1; the GUI (phase 3) may choose one in its own ADR.
- The textured threshold reuses the plan's 0.03. The midtone band (25th to 75th percentiles), the 10% trim and the minimum of 16 midtone cells are controller choices pending measurement; they are marked `TODO(measure)` in the step 5 constants table. The median fallback exists so that a frame with almost no texture (a flat test target, or a badly underexposed frame) still gets a defined `x_0`.
- `Estimates` in `negg-types` carries `anchor` and `x0` (before the user's exposure is added) so the sidecar records both and a render is reproducible without re-running the estimator.
- Percentiles are nearest-rank, as implemented by `percentile` in `negg-film`.

## Eval delta

None measured; the corpus has no frames yet.
