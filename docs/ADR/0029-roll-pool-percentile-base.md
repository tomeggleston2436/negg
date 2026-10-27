# 0029: roll mode does not lift a percentile film base, and pools the cast only from a border base

## Status

Accepted, 2026-10-24. Supersedes in part ADR 0017: the clause that gives every non-excluded frame the pooled Dmin, the gamma rule (`gamma_f = max(Dmax_f - Dmin_pooled, 0.05)`) and the use of the pooled cast by every non-fixed frame. The rest of ADR 0017 stands.

## Context

`docs/MATHS.md` step 3 (roll pooling) gave every frame whose film base is not fixed the roll's pooled Dmin, including frames Rejected from the pool, and step 6 gave every non-fixed frame the roll's pooled cast. Both assume that every frame of the roll measures the same film base.

That holds for frames whose Dmin is a Border estimate: the unexposed border is the base. It does not hold for a frame whose own Dmin is a Percentile estimate (no border visible, for example after a tight crop or on a frame scanned inside its image area). The 0.5th percentile of a frame's density is its thinnest scene density, which can lie well below the thinnest density of other frames on the same roll. Rendering such a frame with a pooled Dmin above its own puts part of its shadows below the base, and the `d >= 0` clamp of step 4 then maps all of them to one flat grey. On real camera scans this was seen on a large share of some frames' pixels.

The pooled cast has the same assumption: it is the cast of the shared base reference. When the pooled Dmin is itself made from percentile estimates, each frame's cast relative to that value differs, and one pooled cast adds a per-frame error instead of removing one.

## Decision

- **Dmin.** In roll mode a frame whose own Dmin source is Percentile never takes a pooled Dmin above its own estimate: per channel it renders with `min(own_c, pooled_c)`. A frame whose own source is Border takes the pooled value as before, Rejected or not (unchanged for Border frames). A fixed (user) film base still wins over everything. The sidecar records `dmin_source = roll` when every channel took the pooled value (ties count as pooled), `percentile` when every channel took the frame's own estimate, and the new value `roll_min` when some channels took one and some the other.
- **Gamma.** `gamma_f = max(Dmax_f - Dmin_f, 0.05)` with `Dmin_f` the Dmin the frame actually renders with: fixed, else the rule above applied to its own estimate and the pooled Dmin. One function (`dmin_with_pool` in `negg-render`) computes that Dmin, and both `pool_roll` (for `gamma_f`, the pooled gamma and the cast fits) and `estimate_frame` (for the render, and for the range gamma when no gamma is pooled) call it, so the pool and the render agree on every frame's base.
- **Cast.** The roll-pooled cast is applied only when the roll's pooled Dmin came from Border frames: a pooled Dmin exists and every Dmin candidate (Inlier or Rejected) is a Border frame, which is what the Border preference of ADR 0017 gives when at least 2 non-excluded frames are Border. Otherwise no cast is pooled (`pool.cast` is `None`), every non-excluded frame's cast role is Unused, and each frame is rendered with its own step 6 fit on its grid inverted with its Dmin and gamma in effect (the fit recorded in the roll sidecar). A fixed cast still wins, auto colour off and a film correction still give the identity. The frame sidecar's `cast_source` records the cast the frame actually used (`fit` or `identity` instead of `roll`).

## Consequences

- A percentile frame on a roll keeps its shadows: its base can only move down towards the pool, never up past its own thinnest density. Moving it down (when its own estimate lies above the pool) is still allowed, because a percentile estimate is biased upward by scene content and the pool is then the better base.
- Border rolls render exactly as before: the same pooled Dmin, gamma and cast for every non-fixed frame, a Rejected outlier included.
- A roll with no border (or only one bordered frame) still pools Dmin and gamma, but each frame removes its own cast. Frames of such a roll can differ more in colour from one another than on a border roll; that is the price of not applying a cast that does not fit them.
- A percentile frame on a border roll (a single cropped frame among bordered ones) takes `min(own, pooled)` per channel and still takes the pooled cast, because the roll's base reference is a true one. It also feeds the cast pool: it is a cast candidate like any frame that is not excluded, not pinned at the density cap and not Rejected from the Dmin or gamma pool, with its own fit made on its grid inverted with its capped Dmin. Its fit is relative to that Dmin, which is at most the pooled one, so it is close to the border frames' fits when its estimate is close to the true base; the weighted median keeps one stray fit from moving the pooled cast. Whether leaving such frames out of the cast pool is better is `TODO(measure)` on corpus rolls (stated in the phase 3 final review, not changed).
- The pooled gamma now comes from the Dmin each frame renders with, so a capped frame contributes its own range and the gamma pool describes the frames as rendered.
- `roll_min` is a new `dmin_source` value. Sidecars written before this change still read; a sidecar with `roll_min` cannot be read by an older negg.
- A roll sidecar written before this change keeps its stored pooled cast; `negg convert` with such a sidecar still applies it until the roll is batched again. The Dmin cap applies at render time and needs no re-batch.

Cost if wrong: Dmin, one branch in `dmin_with_pool`; cast, one condition in `pool_roll`.

## Eval delta

None measured: the committed corpus has no frames, and the eval exits 0 as before. Renders outside roll mode do not change; the `--no-roll` golden render is byte-identical.
