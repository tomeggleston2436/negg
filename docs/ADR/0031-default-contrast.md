# 0031: default contrast: the film base prints at the knee of the toe

## Status

Accepted, 2026-10-24. Reopens ADR 0030 and supersedes it in part: the fixed black point `P_black = 2.0` is replaced by a value derived from the paper constants. The floor itself (`x_0 = max(x_0_anchor, x_black)`), where it is computed and how it meets the user's `exposure` and `cmy_c` stand. ADRs 0006 (grade) and 0008 (anchor) are not changed.

## Context

After ADRs 0026, 0027, 0029 and 0030, renders of real scans still looked a little low in contrast and saturation: lifted shadows, midtones and highlights brighter than the reference renders, and less chroma. Four defaults could close that gap, and each was measured on real scans against reference renders (the corpus is private, so this ADR gives the outcome and not the numbers):

- The default grade `R` (ADR 0006). A harder grade (smaller `R`, larger `k`) brightened the print, because with the floor active the grade pivots around the base; every frame moved further from the references. A softer grade darkened midtones and highlights and lowered the colour difference, but lifted the shadows and lowered chroma.
- The shoulder `a_hl`. A softer shoulder pulled the highlights down and lowered the colour difference, but also lifted the shadows and lowered chroma; a harder shoulder made everything worse.
- The anchor target `P = 0.75` (ADR 0008). A darker target helped some frames and made at least one worse; `P = 0.75` is also the print density of an 18% grey, which is a principled value in its own right.
- The black point `P_black` (ADR 0030). A deeper black improved shadows, midtones, highlights and chroma together on every frame where the floor is active and left the others exactly as they were.
- The toe `a_sh` alone had almost no effect.

Saturation was examined separately. No candidate moved the chroma gap by much; the part of it that remains is mostly the reference's own cool look (blue skies and sea rendered cooler and more saturated), which is not an error to chase. Chroma followed tone where tone moved (a denser print sits further down the straight line of the curve, away from the shoulder that compresses channel differences), so no saturation control was added to the print stage.

`P_black = 2.0` was a provisional choice (`TODO(measure)`) with no derivation. The paper curve has a natural black point of its own: the knee of the toe, where the straight line `v = x` (above the shoulder) meets paper black `Dmax_p`. At the knee the toe's softplus equals `ln 2`, so the print density there is `Dmax_p - ln(2) / a_sh`. Below the knee the toe compresses the shadows into paper black; above it the curve is close to its straight line. Putting the clearest part of the negative exactly at the knee uses the paper's full straight-line range without pushing the base into the compressed toe.

## Decision

Set the black point to the knee of the toe:

```
P_black = Dmax_p - ln(2) / a_sh            # 2.3 - ln(2) / 6 = 2.1845 with the defaults
x_black = Dmin_p + softplus_inv(a_hl * (Dmax_p - Dmin_p)) / a_hl    # 2.2996 with the defaults
x_0     = max(x_0_anchor, x_black)
```

The second line is the ADR 0030 inverse with `v_black = Dmax_p`, which is exact because `softplus_inv(a_sh * (Dmax_p - P_black)) = softplus_inv(ln 2) = 0`. `black_print_density` and `black_point_x0` in `negg-render` compute it; the constant `BLACK_PRINT_DENSITY` is removed. It is derived from the paper constants (`Dmax_p`, `a_sh`, `Dmin_p`, `a_hl`), so it has no constant of its own and no `TODO(measure)`; choosing the knee as the definition of black is a design choice, supported by the measurement above.

## Consequences

- With the defaults the base prints at `P = 2.1845` instead of 2.0, and the floor is active for every anchor above about `e = -0.632` instead of `-0.525`. The default grade then maps the negative's normalised span `e` in `[-1, 0]` to `P` from 0.179 to 2.1845 (before: 0.118 to 2.0): the base is darker, the midtones of a floored frame are denser (`e = -0.5` prints at 1.056 instead of 0.806) and the densest highlights stay above paper white (0.06), so they keep detail. No pixels clipped at L* 99.5 or above in the measured renders before or after.
- Frames whose anchor is below `e = -0.632` print exactly as before, byte for byte; the change only reaches frames where the floor is active.
- The floor is now defined for every paper that passes `Settings::validate` (`paper_dmax > paper_dmin`, positive shoulder and toe). Under ADR 0030 a paper with `paper_dmax <= 2.0` turned the floor off; now a soft paper is floored at its own knee. A user who lowers `toe` (a softer toe, smaller `a_sh`) lowers `P_black = Dmax_p - ln(2) / a_sh`, so the base prints lighter at the same `x_0` (`x_black` does not depend on `a_sh`); one who lowers `paper_dmax` moves the knee down with paper black. The floor is inactive whenever `P_black` is at or below the anchor's print density (0.75 by default), because then `x_black <= x_0_anchor`.
- No setting default changed, so the settings hash and the sidecar schema are unchanged. A sidecar written before this change keeps its stored `x0`; re-rendering re-solves it.
- Tests in `negg-render` that fail before this change: the default wedge `e = 0, -0.25, -0.5, -0.75, -1` with the floor active prints at `P = 2.1845, 1.6680, 1.0559, 0.5088, 0.1790`; `black_print_density` is `Dmax_p - ln(2) / a_sh` and the closed form of `x_black` equals the general inverse of the curve for several papers; `x_black = 2.2996` with the defaults; a paper with `paper_dmax = 1.9` is floored at its own knee. The property tests (base at least at `P_black`, anchor at least at 0.75, monotone and neutral with the floor, kernel against the written equations) pass with the derived `P_black`.
- **Golden re-baseline (rule of ADR 0027).** `crates/negg-cli/tests/data/golden.phase1.negg.tif` was regenerated with the release binary of this change (`negg convert --no-roll` on the deterministic input written by the ignored `write_the_golden_input` helper), with the test's ICC-date masking unchanged. The golden input's anchor is -0.1978, so the floor is active: `x_0` rises from 2.0292 to 2.2996 and the base prints at `P = 2.1845` instead of 2.000. Every pixel changes (3072 of 3072, all 9216 samples); the largest change is 8099 of 65535 code values and the mean absolute change is 4066. The corner (base) pixel goes from 6544 to 4898 in each channel.
- Cost: none; the floor is still two scalar expressions per frame.

## Eval delta

None measured on the committed corpus: it has no frames (`negg eval` exits 0 with 0 frames). Measured on real scans against reference renders, outside the repository: the mean CIEDE2000 gap fell, no frame got worse, and the shadow, midtone and highlight L* gaps and the chroma gap all narrowed; frames whose anchor already printed the base beyond the new black point were unchanged.
