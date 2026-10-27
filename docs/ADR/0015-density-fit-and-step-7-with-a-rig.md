# 0015: camera-to-density fit, its use, step 7 and the CSV formats

## Status

Proposed, 2026-10-23.

## Context

`docs/MATHS.md` step 2 says the camera-to-density fit is "stored in the rig profile" but not how it is fitted without ambiguity, how it is consumed (the former `TODO(spec)`), or whether it changes step 7 (the other `TODO(spec)`). The original model `D_c = a_c + b_c log10(I_c) + sum_k c_ck log10(I_k)` is over-parameterised on the diagonal: `b_c` and `c_cc` multiply the same regressor. A neutral silver wedge also makes the three log channels collinear, so the cross terms are unidentifiable from it.

## Decision

- Density fit: the stored model is `D_c = a_c + sum_k G_ck log10(max(I_k, 1e-6))` with `b_c = G_cc`, `c_ck = G_ck` for `k != c` and `c_cc = 0`. The patch value `I` is `patch_median` of the rig-corrected (flat, unmix) wedge grid; the known `D` comes from `wedge.csv`. Fit per channel by least squares (nalgebra SVD). Use the full model when `N >= 8` and the condition number of the centred `N x 3` design matrix of `log10 I` is at most 100; otherwise the diagonal model (`c = 0`, fit `a_c` and `b_c`) when `N >= 3`, with `model = "diagonal"` and a warning; otherwise error. Error when any `b_c >= 0`. `residual_rms` is `sqrt(mean of the 3N squared residuals)`, in log-D. The condition number is used only to choose the model and to name the reason in the diagonal-fallback warning; the rig profile schema has no field for it, so it is never written.
- Density use: with a density model, step 3 density is the model, both on the grid (Dmin, Dmax, cast, anchor) and per pixel in `render_full`; without one it is `-log10(max(I, 1e-6))`. `a_c` cancels in step 4 (Dmin subtraction) but is kept so sidecar densities are wedge-referenced. The "grey card on film whose densities are estimated from the roll" source for the fit is deferred: phase 2 fits from a wedge only.
- Step7: a rig profile does not change step 7. The step 7 entry matrix is still applied to `out_c`, unchanged by flat field, unmix or the density model. Since ADR 0026 that matrix is the BT.2087 Rec.709-to-Rec.2020 matrix for RAW input (the camera matrix of ADR 0012 is no longer applied) and the profile's colorant matrix for a tagged TIFF.
- Wedge csv: `wedge.csv` is UTF-8, comma separated, no quoting; blank lines and lines starting with `#` are ignored; the first other line is the header, exactly `patch,x,y,width,height,density` (neutral wedge) or `patch,x,y,width,height,density_r,density_g,density_b`. `x`, `y`, `width`, `height` are fractions of the frame as in the manifest `grey_card` (a rect inside [0, 1]); `patch` is a unique label; densities are certified transmission densities, finite and >= 0. Any malformed line is an error naming the line number. `patches.csv` for `profile film` has the header exactly `file,x,y,width,height`, with `file` a file name in the roll directory.

## Consequences

- Only coloured patches (per-channel densities) can fit the cross terms; a neutral wedge falls back to the diagonal model and says so. The thresholds `N >= 8`, condition number 100 and `N >= 3` are Design choices, `TODO(measure)`.
- The identity model is `a = 0`, `G = -I`, which equals the plain density formula, so a rig without `[density_fit]` and a rig with an identity fit render identically.
- The step 7 decision rests on no phase 2 measurement characterising the colorimetry of the positive. A wedge constrains only the grey axis, and white normalisation already maps neutrals to neutrals, so Gate 2 neutrality does not depend on this choice.
- The grey-card-on-film fit source is deferred to a later phase.

Cost if wrong: density fit, the thresholds; density use, an extra fit path later; step 7, saturation and hue change, not neutrality; wedge csv, a parser change.

## Eval delta

None, no render change without a rig profile. The step 7 decision keeps step 7 byte-identical.
