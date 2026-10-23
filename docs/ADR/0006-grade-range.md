# 0006: `range` in the grade formula

## Status

Proposed, 2026-10-20 (controller ruling pending owner review).

## Context

`docs/MATHS.md` step 5 expresses the grade `k` to the user as an ISO-R style number `R` (default 115, range 50 to 180) with

```
k = 2.9 * range / (R/100)
```

clamped to [2, 10]. The plan (`IMPLEMENTATION_PLAN.md`, step 5) gives the formula in exactly that form and does not say what `range` is. `docs/MATHS.md` therefore carried the line "TODO(spec): the plan does not define `range` in the grade formula. Until it does, the default `k` for `R = 115` cannot be computed and the formula must not be implemented from a guess." Phase 1 cannot land `negg-render` without a value, so this ADR records the ruling (contract item R-grade).

## Decision

`range` is the normalised log-exposure span of the inverted negative from step 4, which is 1.0 by construction because each channel of `e_c` spans [0, -1]. The formula therefore reduces to `k = 290 / R`, clamped to [2, 10]. Implement `grade_k(R)` in `negg-render` as exactly that.

## Consequences

- The defaults become computable: `k(115) = 2.52` (2.5217 unrounded), `k(50) = 5.8`, `k(180) = 2.0` after clamping from 1.611. The lower clamp is active for `R` above 145, so the top of the user's grade range is flat; the upper clamp of 10 is never reached within 50 to 180.
- The alternative reading, `range = Dmax_p - Dmin_p = 2.24` (the paper's density range with the step 5 defaults), gives `k(115) = 5.65`. With `e_c` spanning [0, -1], that `k` maps the negative's full span onto 5.65 log-D of print exposure against a paper that only has 2.24 log-D of range, so most of the negative's tonal range would be pushed past the shoulder or the toe at the default grade. It was rejected because it clips much of the negative's range at the default setting, which is the opposite of what a default should do.
- The constant 2.9 is kept as the plan wrote it, but the plan cites no source for it. With `range = 1.0` it is simply the scale that makes `R = 115` give a `k` near 2.5; it is a controller choice pending measurement against the corpus and is marked `TODO(measure)` in the step 5 constants table.
- Because `range` is 1.0 by construction, `k` depends only on the user's `R`. When a film profile or neutral fit replaces the fallback gamma in step 4 (phase 2) the span of `e_c` is no longer exactly [0, -1]; that does not change `k`, and absolute contrast remains a user choice via `R`, as the plan says.
- `PrintCoefficients::new` in `negg-render` folds `k` into the per-channel coefficients; the property test required by step 5 checks the folded form against the four equations.

## Eval delta

None measured; the corpus has no frames yet.
