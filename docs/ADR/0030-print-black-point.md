# 0030: a black point in the print stage

## Status

Accepted, 2026-10-24. Reopens ADR 0008 and supersedes it in part: `x_0` is no longer set by the exposure anchor alone. The anchor, its analytic solve and the user's `exposure` and `cmy_c` stand.

Superseded in part by ADR 0031: `P_black` is no longer the fixed 2.0 but the knee of the toe, `Dmax_p - ln(2) / a_sh` (2.1845 with the defaults), so the floor is defined for every valid paper. The floor `x_0 = max(x_0_anchor, x_black)` and its interaction with `exposure` and `cmy_c` stand.

## Context

`docs/MATHS.md` step 5 sets the print exposure offset `x_0` only from the exposure anchor (ADR 0008): the trimmed mean luma of the textured midtone cells prints at `P = 0.75`. Nothing ties the film base to black. The film base is `e = 0` after step 4, the clearest part of the negative, and it should print as the darkest black the print holds.

When the anchor sits high (close to `e = 0`) the solved `x_0` is small and the base prints well short of paper black. This happens when the textured midtones are themselves thin, for example a frame whose flat sky is excluded from the anchor and whose textured cells are shadowed foreground. The print then has lifted, grey blacks and its midtones are too bright as a consequence. Measured on real scans against reference renders, this was the main cause of washed-out renders, and flooring the base at a print density of 2.0 closed most of the tone gap to the references and the midtone brightness with it.

## Decision

Floor `x_0` so that the film base prints at least at `P_black = 2.0`:

```
v_black = Dmax_p - softplus_inv(a_sh * (Dmax_p - P_black)) / a_sh
x_black = Dmin_p + softplus_inv(a_hl * (v_black - Dmin_p)) / a_hl
x_0     = max(x_0_anchor, x_black)
```

`x_0_anchor` is the ADR 0008 solve. At `e = 0` the paper exposure is `x = x_0`, so the grade drops out and `x_black` is the closed-form inverse of the print curve at `P_black`; it depends on the print settings only, never on scene content. `P_black = 2.0` is a provisional choice, `TODO(measure)`, named `BLACK_PRINT_DENSITY` in `negg-render`. The floor is computed in one place (`solve_exposure` in `negg-render`, called by `estimate_frame`), so single-frame and roll renders both use it.

How it meets the user's settings:

- `exposure` and `cmy_c` are added after the floor, as they were added after the anchor. An explicit exposure therefore still moves the whole print, base included: a negative exposure lifts the base below `P_black` on purpose.
- Black point compensation (`print.black_point_compensation`), when enabled, acts on `out_c` after the floor, so the base lands darker than the floor's target `P_black`.
- The grade `k` does not enter `x_black`. With the floor off the grade pivots around the anchor as before; with it on, around the base.
- The paper constants (`paper_dmin`, `paper_dmax`, `shoulder`, `toe`) enter `x_black`. When `P_black` is unreachable (`paper_dmax <= 2.0`) the floor is off. When the anchor is unreachable `x_0` stays not finite and the frame is rejected, as before; the floor never hides that error.
- No setting fixes `x_0` or the anchor directly, so there is no override to defer to beyond `exposure`.

## Consequences

- With the step 5 defaults `x_black = 2.0292`, and the floor is active whenever the anchor lies above about `e = -0.525`. Frames whose anchor is lower print exactly as before, byte for byte.
- When the floor is active the anchor prints darker than `P = 0.75`. The anchor stays the exposure for normally exposed frames; the floor only stops a high anchor from lifting the blacks.
- `Estimates.x0` is the floored value; the anchor's own `x_0` is not stored. No field was added to `Estimates`, so no sidecar schema bump is needed (ADR 0010 requires one for an added field). Whether the floor was active is reported by `FrameRender::black_point_floor` and as `black point floor: on` or `off` in `--dump-stats`, so a user can see why a frame printed darker. A sidecar written before this change keeps its stored `x0`; re-rendering re-solves it.
- Tests in `negg-render` that fail before this change: a synthetic scene of a flat dense sky over a textured foreground near the base (anchor -0.208) prints its base at `P = 2.0` within 2e-3, against 1.24 with the anchor alone; property tests check that the base prints at least at `P_black`, the anchor at least at 0.75, and that the floored print is monotone and neutral for every grade, paper curve and anchor in range. A scene whose anchor already prints the base beyond `P_black` (anchor -0.79) renders byte-identical to the anchor-only render.
- **Golden re-baseline (rule of ADR 0027).** `crates/negg-cli/tests/data/golden.phase1.negg.tif` was regenerated with the release binary of this change (`negg convert --no-roll` on the deterministic input written by the ignored `write_the_golden_input` helper), with the test's ICC-date masking unchanged. The golden input's anchor is -0.1978, so the floor is active: `x_0` rises from 1.2037 to 2.0292 and the base prints at `P = 2.000` instead of 1.214. Every pixel changes (3072 of 3072, all 9216 samples); the largest change is 23484 of 65535 code values and the mean absolute change is 14522. The corner (base) pixel goes from 17965 to 6544 in each channel.
- The 24 MP release render (`render_frame`, perf test) takes 804 ms, against about 800 ms before: the floor is two scalar solves per frame.

## Eval delta

None measured: the committed corpus has no frames (`negg eval` exits 0 with 0 frames). On real scans against reference renders, outside the repository, the floor reduced the mean CIEDE2000 gap and most of the mean absolute L* gap (see Context).
