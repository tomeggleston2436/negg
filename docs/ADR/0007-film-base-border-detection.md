# 0007: when the film base border path is trusted

## Status

Proposed, 2026-10-20.

## Context

`docs/MATHS.md` step 3 estimates `Dmin_c` from the median of the flat cells (std/mean below 0.03) in the outer 15% band of the analysis grid when the frame has a detectable unexposed border, and from the 0.5th percentile of `D_c` over the grid otherwise. The original model says "if the frame has a detectable unexposed border" and no more. `docs/MATHS.md` carried the line "TODO(spec): the model does not define what makes a border 'detectable', nor the minimum number of flat cells required before the border path is trusted over the percentile path." `estimate_dmin` in `negg-film` needs the rule, so this ADR records the decision.

## Decision

Use the border path only when both of the following hold, otherwise use the percentile path: (a) at least 20 cells, and at least 5% of the cells in the outer 15% band, are flat (std/mean below 0.03); and (b) the per-channel median density of those flat cells is at most 0.05 log-D above the 0.5th-percentile density of the whole grid, in every channel. The estimator reports which path was used (`DminSource::Border` or `DminSource::Percentile`) and the number of flat border cells it found.

## Consequences

- Condition (a) stops a handful of flat cells (a patch of plain wall that happens to touch the band) from being taken as a border. Condition (b) encodes what an unexposed border physically is: the least dense region on the negative. A flat wall or an even sky is flat but denser than base, so it fails (b) and the frame falls through to the percentile path instead of getting a Dmin that is too high.
- When the border path is taken, the result is the median of flat border cells, so by (b) it lies within 0.05 log-D of the percentile estimate. The two paths therefore never disagree by more than that tolerance; the border path is preferred because it measures base directly rather than assuming the darkest 0.5% of the image is base.
- The numbers 20 cells, 5% and 0.05 log-D are provisional choices pending measurement against the corpus. They are marked `TODO(measure)` in the step 3 constants table and must not be tuned without a corpus delta.
- The flat threshold 0.03 is the original model's own number and is unchanged. The same threshold, inverted, defines a "textured" cell for the exposure anchor (ADR 0008).
- `DminEstimate` in `negg-film` carries `source` and `flat_border_cells`, and the sidecar (ADR 0010) records the source, so `negg-cli eval` can separate border-derived and percentile-derived frames when the corpus exists.
- Phase 2 roll pooling takes the per-frame values as they are; it does not need to know which path produced them.

## Phase 3 amendment

Amended by ADR 0021 (Accepted): when `geometry.crop` is set, the border search above runs only inside the crop rect, unbuffered. The outer 15% band is the outer 15% of the crop grid, not of the whole frame. A tight crop therefore usually loses the border source, and Dmin falls back to the 0.5th percentile of the crop grid (`dmin_source = percentile`); a crop that keeps some rebate keeps the border source. The thresholds (20 cells, 5%, 0.05 log-D) and the guard are unchanged. With an analysis area (ADR 0021 amendment) the search runs inside the unbuffered analysis area instead, which takes precedence over the crop for the analysis. With neither the behaviour is exactly as above.

## Eval delta

None measured; the corpus has no frames yet.
