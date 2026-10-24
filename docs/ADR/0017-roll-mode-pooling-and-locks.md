# 0017: roll mode, pooling and per-frame locks

## Status

Proposed, 2026-10-23 (controller rulings 47 to 50 and 59 pending owner review).

Superseded in part by ADR 0029 (Rulings 105 and 106): a frame whose own Dmin is a Percentile estimate never takes a pooled Dmin above its own (amends Ruling 59), `gamma_f` uses the Dmin each frame renders with (amends the gamma rule of Ruling 48), and the pooled cast is applied only when the pooled Dmin came from Border frames (amends Ruling 49).

## Context

`docs/MATHS.md` steps 3, 4 and 6 say that Dmin, gamma and the cast fit are pooled across a roll with outlier rejection, and the plan asks for per-frame lock and override with `RollSettings` inheritance. The plan does not define what a roll is, how frames are chosen as candidates, the confidence weight and inlier rule for the cast pool (a `TODO(spec)` in step 6), or how a user lock interacts with the pool.

## Decision

- Ruling 47 (R-roll): a roll is the set of batch inputs of one directory (`negg batch <dir>`, the `is_batch_input` rule). The roll sidecar is `<dir>/roll.negg.json`; `convert_frame` refuses a frame whose sidecar path equals it. `negg batch` is roll mode unless `--no-roll`. `negg convert` uses an existing roll sidecar's settings and pool when the frame is listed in it, unless `--no-roll`, and never rewrites it. In the eval, a roll is the manifest `roll` field.
- Ruling 48 (R-pool): per quantity each frame has a role Inlier, Rejected, Excluded (user-fixed in its resolved settings) or Unused (not a candidate). Dmin candidates are the frames with `dmin_source == Border` when at least 2 non-excluded frames are Border, else all non-excluded frames. The pool is the per-channel median (`negg_film::median`) over candidates; reject a frame when any channel lies more than 0.3 log-D from that median (Chebyshev distance); the pooled value is the per-channel median of the inliers. No inliers means no pooled value, and frames keep their own with a warning. Gamma: per frame `gamma_f = max(Dmax_f - Dmin_pooled, 0.05)` (own Dmin if none is pooled), pooled by the same rule over non-excluded frames. The inlier frames used by Gate 2 are the Dmin inliers. Ruling 59: a frame Rejected from a pool still renders with the pooled value; only Excluded (user-fixed) frames keep their own. When no pooled value exists, every frame keeps its own.
- Ruling 49 (R-cast-pool): each non-excluded frame's cast is fitted (`fit_cast`) on its grid inverted with the pooled Dmin and gamma (and film correction offsets). Candidates are the frames not Rejected for Dmin or gamma with `cells_used > 0`. The confidence weight is `cells_used`. Each of `slope_r`, `offset_r`, `slope_b`, `offset_b` is the weighted median of its own values (sort ascending, first value where cumulative weight >= half the total); the pooled `cells_used` is the sum of candidate weights. No candidates means no pooled cast. Excluded: `colour.cast` fixed or `auto_colour` off.
- Ruling 50 (R-lock): a frame's overrides live in its sidecar as `FrameOverrides` (every field `Option`, `None` meaning inherit from `RollSettings::shared`). `resolve(roll, overrides)` is `shared` with each `Some` field replaced. Precedence per estimate: settings Fixed, then film correction (gamma, offsets), then roll pool, then frame estimate. Fixed frames (lock or override) neither feed nor take that pool. `negg lock <frame>` writes the frame's current Dmin, gamma and cast estimates into its sidecar overrides as Fixed (`--dmin`, `--gamma`, `--cast` select; none selects all) and also rewrites the sidecar's resolved settings and their hash, so the sidecar stays consistent with the image it describes.

## Consequences

- Preferring Border Dmin keeps a frame with no usable border (a percentile estimate) from dragging the pool, while still working on a roll with no borders at all.
- The 0.3 log-D reject distance is the plan's value; the gamma floor of 0.05 reuses the step 4 floor. The "at least 2 Border frames" rule is a Controller choice, `TODO(measure)`.
- The roll pool is computed in two passes. Pass 1 decodes one frame at a time, applies the rig's step 2 corrections and keeps only the frame's analysis; the frames are pooled; pass 2 decodes each frame again, one at a time, and renders it with the pooled values. Only the analyses, not the images, are held across frames, so memory stays bounded. A frame that fails in pass 1 is reported and left out of the pool and pass 2.
- A locked frame is a reproducible record: re-batching keeps its locked values and it does not move the pool.
- Locks and overrides are never lost to a damaged sidecar: when a frame's existing sidecar cannot be read or parsed, `negg convert` and `negg batch` fail that frame (counted as failed in the batch summary) and leave its sidecar and output untouched. A missing sidecar means no overrides.
- A stored pool is only valid in the density space it was computed in. `negg convert` uses the roll sidecar's pool only when the effective `profiles` and `pool_*` flags equal the roll sidecar's (Ruling 63); otherwise it warns that the roll must be batched again and renders the frame with the roll settings and its overrides but without the pool, and the frame's sidecar records no `roll`.
- An outlier Rejected from the pool is rendered with the pooled value (Ruling 59), consistent with the pool describing the roll. A user who wants such a frame to keep its own estimate locks it with `negg lock`, which makes it Excluded.
- With a film correction no cast is pooled: the cast is the identity (Ruling 45), so the "film correction offsets" clause of Ruling 49 has no effect at render.
- Weighting the cast median by `cells_used` favours frames with more neutral evidence; a different weight is a one-line change.

Cost if wrong: Ruling 47, none beyond file naming; Ruling 48, the candidate filter; Ruling 49, the weight definition; Ruling 50, one more `ChannelEstimate` variant; Ruling 59, one branch in the render driver.

## Eval delta

None, no render change outside roll mode. Roll mode changes eval roll renders; the corpus has no frames yet.
