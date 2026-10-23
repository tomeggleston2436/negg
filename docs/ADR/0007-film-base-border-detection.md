# 0007: when the film base border path is trusted

## Status

Proposed, 2026-10-20 (controller ruling pending owner review).

## Context

`docs/MATHS.md` step 3 estimates `Dmin_c` from the median of the flat cells (std/mean below 0.03) in the outer 15% band of the analysis grid when the frame has a detectable unexposed border, and from the 0.5th percentile of `D_c` over the grid otherwise. The plan says "if the frame has a detectable unexposed border" and no more. `docs/MATHS.md` carried the line "TODO(spec): the plan does not define what makes a border 'detectable', nor the minimum number of flat cells required before the border path is trusted over the percentile path." `estimate_dmin` in `negg-film` needs the rule, so this ADR records the ruling (contract item R-border).

## Decision

Use the border path only when both of the following hold, otherwise use the percentile path: (a) at least 20 cells, and at least 5% of the cells in the outer 15% band, are flat (std/mean below 0.03); and (b) the per-channel median density of those flat cells is at most 0.05 log-D above the 0.5th-percentile density of the whole grid, in every channel. The estimator reports which path was used (`DminSource::Border` or `DminSource::Percentile`) and the number of flat border cells it found.

## Consequences

- Condition (a) stops a handful of flat cells (a patch of plain wall that happens to touch the band) from being taken as a border. Condition (b) encodes what an unexposed border physically is: the least dense region on the negative. A flat wall or an even sky is flat but denser than base, so it fails (b) and the frame falls through to the percentile path instead of getting a Dmin that is too high.
- When the border path is taken, the result is the median of flat border cells, so by (b) it lies within 0.05 log-D of the percentile estimate. The two paths therefore never disagree by more than that tolerance; the border path is preferred because it measures base directly rather than assuming the darkest 0.5% of the image is base.
- The numbers 20 cells, 5% and 0.05 log-D are controller choices pending measurement against the corpus. They are marked `TODO(measure)` in the step 3 constants table and must not be tuned without a corpus delta.
- The flat threshold 0.03 is the plan's own number and is unchanged. The same threshold, inverted, defines a "textured" cell for the exposure anchor (ADR 0008).
- `DminEstimate` in `negg-film` carries `source` and `flat_border_cells`, and the sidecar (ADR 0010) records the source, so `negg-cli eval` can separate border-derived and percentile-derived frames when the corpus exists.
- Phase 2 roll pooling takes the per-frame values as they are; it does not need to know which path produced them.

## Eval delta

None measured; the corpus has no frames yet.
