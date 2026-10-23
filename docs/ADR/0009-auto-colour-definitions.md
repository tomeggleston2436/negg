# 0009: luma, chroma, hue and the applied correction in auto colour

## Status

Proposed, 2026-10-20 (controller ruling pending owner review). Amended 2026-10-21 (Ruling 30, pending owner review): luma and hue are read on `-e`; see "Amendment" below.

## Context

`docs/MATHS.md` step 6 fits a cast correction on the normalised log image `e_c` from step 4, on the analysis grid: split the cells into luma bands, keep the lowest-chroma cells outside the foliage and sky hue sectors, fit R and B against G as lines, and apply per-channel log-affine corrections before step 5. The plan uses the words luma, chroma and hue without defining them on `e_c`, and says "apply as per-channel log-affine corrections" without writing the correction down. `docs/MATHS.md` carried the line "TODO(spec): the plan does not define luma, chroma or hue on the normalised log image, nor the exact form of the applied correction (the implied form is to invert the fitted line so that the selected cells land on `e_G`, but this is not written down). Both need a decision before the first render lands." `fit_cast` and `apply_cast` in `negg-render` need the definitions, so this ADR records the ruling (contract item R-colour). The phase 2 roll mode TODO is not touched.

## Decision

Work on `e = (e_R, e_G, e_B)` per grid cell, each in [-1, 0]. Define luma `l = (e_R + e_G + e_B) / 3`, chroma `c = max(e) - min(e)`, and hue as the standard HSV hue angle of the triple with the usual 60-degree sector formula (red at 0, green at 120, blue at 240 degrees); hue is undefined when `c < 1e-4`; such a cell cannot fall in an excluded hue sector and is kept in the selection. The three luma bands are shadows `[p0, p20)`, mids `[p20, p80)` and highlights `[p80, p100]` of `l` over the grid; the 50th percentile is the centre of the mids band, not a boundary. Within each band, first exclude cells whose hue lies in [90, 160] or [200, 260] degrees, then keep the lowest-chroma 30% of what remains. Pool the kept cells of the three bands and fit, by ordinary least squares, `e_R = s_R * e_G + o_R` and `e_B = s_B * e_G + o_B`. Clamp `s_R` and `s_B` to [0.8, 1.2]. Apply the correction to the full-resolution `e` image before step 5 as `e_R <- (e_R - o_R) / s_R` and `e_B <- (e_B - o_B) / s_B`, so that the selected cells land on `e_G`. If fewer than 8 cells survive selection, the correction is the identity (slopes 1, offsets 0) and the estimator says so.

## Consequences

- The HSV sector formula, written out for a triple whose maximum channel is `e_R`, `e_G` or `e_B` respectively, is `h = 60 * (((e_G - e_B) / c) mod 6)`, `h = 60 * ((e_B - e_R) / c + 2)`, `h = 60 * ((e_R - e_G) / c + 4)`. It depends only on channel differences, so it is well defined on values in [-1, 0]; the `c < 1e-4` floor is what keeps the division finite.
- Luma is the plain channel mean, not a weighted sum. After step 4 a neutral has `e_R = e_G = e_B`, so any weighting would only matter for coloured cells, which are exactly the ones the selection discards. Chroma as the max-min spread is the smallest definition that is zero on neutrals and monotone in cast size.
- Hue exclusion runs before the 30% chroma cut, so the 30% is taken from the non-foliage, non-sky cells of each band; a band that is all grass or all sky contributes nothing rather than its least green or least blue cells.
- The fit is pooled across the three bands rather than fitted per band; the bands shape the selection, so every tonal region is represented, but one affine line per channel is what step 6 applies and so one line is what is fitted.
- `(e_R - o_R) / s_R` is the exact inverse of the fitted line, so a cell on the line is mapped onto `e_G`. The slope clamp [0.8, 1.2] is the plan's "plus or minus 0.2" written as an interval; when a fitted slope is clamped, the offset is recomputed through the centroid of the pooled cells, `o = mean(e_target) - s_clamped * mean(e_G)` (with `e_target` the `e_R` or `e_B` values of those cells), so the selected cells still land on `e_G` on average.
- Recomputing the offset after a clamp is what keeps the clamp from shifting the whole channel: a clamped slope with the original offset would move the centroid of the selected cells off `e_G`, which is a cast of its own.
- Cells with `c < 1e-4` are the most neutral cells available, so they are kept: the hue exclusion cannot apply to them and the 30% chroma cut naturally favours them.
- The chroma floor 1e-4 and the minimum of 8 surviving cells are controller choices pending measurement and are marked `TODO(measure)` in the step 6 constants table. The band percentiles, the 30% fraction, the hue sectors and the slope clamp are the plan's own numbers.
- `CastCorrection` in `negg-types` carries the two slopes, the two offsets and `cells_used`; identity values together with `cells_used` below 8 show in the sidecar that the fallback was taken.
- Percentiles are nearest-rank, as implemented by `percentile` in `negg-film`.

## Amendment (2026-10-21, Ruling 30)

ADR 0011 fixed the sign of the print stage so that a more negative `e_c` prints brighter in channel `c`. Read on `e` itself, the hue sectors of this ADR were then the complement of what they mean: a frame of grass, which prints green, has `e_G` as its most negative channel, so its hue on `e` is magenta (about 300 degrees) and it passed the foliage exclusion; its least saturated cells then dominated the fit and turned a grey patch magenta-green by C*ab 48.7 in the phase 1 review's foliage scene. Likewise the luma bands called the cells that print brightest "shadows".

Decision, amended: compute luma and hue on the print-space orientation `p = -e` of each cell. Luma is `l = (p_R + p_G + p_B) / 3`, so the shadows band `[p0, p20)` holds the cells that print darkest; hue is the HSV hue angle of `p`, so the foliage sector [90, 160] and the sky sector [200, 260] hold cells that print green and blue. Chroma, `max - min`, is the same on `e` and on `p`. The fit and the applied correction are unchanged and stay on `e`: `e_R = s_R * e_G + o_R`, `e_B = s_B * e_G + o_B`, `e_R <- (e_R - o_R) / s_R`, `e_B <- (e_B - o_B) / s_B`. The Consequences above read with `p` in place of `e` for luma and hue.

Consequences of the amendment: `fit_cast` in `negg-render` reads luma and hue on `-e`; a unit test fits a grid of neutral greys among print-green foliage to the identity within 1e-3 (it fitted offsets of 0.04 and 0.025 before), and the phase 1 review's foliage scene renders its grey patches neutral. Cost if wrong: one sign.

## Eval delta

None measured; the corpus has no frames yet.
