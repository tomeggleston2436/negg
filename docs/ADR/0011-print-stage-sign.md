# 0011: sign of the print stage exposure term

## Status

Proposed, 2026-10-20.

## Context

Step 5 of the original model, as first transcribed into `docs/MATHS.md`, writes the print exposure as

```
x_c = -log10(T_c) * k + x_0 + cmy_c
```

With step 4's own definitions, `d_c = D_c - Dmin_c >= 0`, `e_c = -d_c / gamma_c` and `T_c = 10^(e_c)`, a bright scene area gives a dense negative, so `T_c` is smallest there. `-log10(T_c) = d_c / gamma_c` then rises with negative density, and the print stage as written maps the brightest scene area to the highest print density. Worked through with the step 5 defaults and an anchor `a = -0.5`: `d/gamma = 0` (film base, the darkest scene area) gives `out = 0.78`, and `d/gamma = 1` (Dmax, the brightest scene area) gives `out = 0.011`. The chain as written renders a negative, not a positive. This came up in review of the phase 1 `negg-render` work against ADR 0008, whose solve for `x_0` inherited the same sign.

## Decision

Step 5 reads

```
x_c = log10(T_c) * k + x_0 + cmy_c
```

equivalently `x_c = e_c * k + x_0 + cmy_c`. The paper's exposure is the negative's transmittance: a dense negative gives low paper exposure, low print density and a bright print. `x_0` is solved as `x_0 = x_target - a * k`, with `x_target` as in ADR 0008.

## Consequences

- `print_pixel` is non-increasing in `e_c`: `e_c = 0` (film base) is the darkest print value and `e_c = -1` (Dmax) the brightest. With the defaults and `a = -0.5`, `x_0 = 1.966`, `e = 0` gives `out = 0.011`, `e = -1` gives `out = 0.78`, and the anchor itself gives `out = 10^-0.75 = 0.178`, which is the property test for the sign and for the solve.
- The user's `exposure` control still adds to `x_0`, with positive meaning more print density, that is a darker print. `cmy_c` is unchanged. Nothing else in step 5 changes: the two softplus terms, the paper constants, black point compensation and the grade of ADR 0006 are as they were.
- ADR 0008 keeps its anchor definition and its `x_target` solve; only the last line of its solve, `x_0 = x_target + a * k`, and its statement of the anchor equation `x = -a * k + x_0` are superseded by this ADR's sign. ADR 0008 should be amended to match when this ADR is accepted.
- The original formula is a sign error against its own step 4, not a second convention. It is recorded here rather than silently corrected, because a disagreement in the model is a decision and not a patch; until this ADR is Accepted the code follows it, because the alternative renders negatives.
- If the original sign were kept instead, step 4 would have to define `e_c` with the opposite sign, which changes the span of `e_c` to [0, 1] and touches ADRs 0006, 0008 and 0009; that is the more invasive fix and is why it was not chosen.

## Eval delta

None measured; the corpus has no frames yet.
