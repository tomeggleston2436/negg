# 0016: patch statistic, neutral-patch fit, absolute contrast and film profiles

## Status

Proposed, 2026-10-23.

## Context

`docs/MATHS.md` step 4 describes a neutral-patch fit giving the ratios `gamma_R/gamma_G` and `gamma_B/gamma_G` and offsets, and leaves open the absolute contrast, where the offsets apply and the default `gamma_G` (a `TODO(spec)`). `docs/PROFILES.md` left open whether a film profile's `[base]` and `[dmax]` override per-frame estimates and where profiles are found. The plan also requires `negg profile film` to derive a film profile from a roll with a grey card frame.

## Decision

- Patch: `patch_median(grid_image, rect)` is the per-channel median over the grid cells whose centres lie inside `rect` shrunk by 10% of its width and height on each side; `None` when fewer than 4 cells qualify. It serves wedge steps, band responses and neutral patches.
- Neutral fit: inputs are the densities above base `d_n = max(D_n - Dmin, 0)` (`D` from `patch_median` of the grid density, `Dmin` the roll-pooled value) for `N >= 1` grey patches. Only when exactly one grey patch is given (`N == 1`) is the film base itself added as a second patch `d = (0, 0, 0)`, since unexposed film is neutral by construction of step 4; with `N >= 2` the patches alone are fitted (amending the original rule that always added the base). For `c` in {R, B}: OLS of `y_n = d_G,n / gamma_G` on `x_n = d_c,n` gives slope `s_c = 1/gamma_c` and intercept `o_c`; `ratio_c = gamma_c / gamma_G = 1 / (s_c * gamma_G)`. The stored offset in log-D is `offset_c = o_c * gamma_G` (G entries 1 and 0). Error when the maximum `d_G` over the points used is below 0.1 log-D or a slope is <= 0. With `P` points used (`P = 2` when `N == 1`, else `P = N`), `residual_rms = gamma_G * RMS of the 2P e-space residuals`, in log-D, and `NeutralFit.patches = P`. One grey card frame therefore suffices, and two patches give the exact solution.
- Contrast: absolute contrast is the green range, `gamma_G = Dmax_G - Dmin_G` of the frame, or the roll-pooled `gamma_G` in roll mode; `gamma_c = ratio_c * gamma_G`. Offsets apply in step 4 as `e_c = -d_c / gamma_c - offset_c / gamma_G`, before step 6. `Estimates::neutral_offset` stores the e-unit value `offset_c / gamma_G`. When a film correction is in use the step 6 cast fit is skipped (identity, `cast_source = Identity`) because the measured neutral fit already removes the same log-affine error.
- Film profile: `negg profile film` writes `[stock]`, `[base]` (roll-pooled Dmin), `[dmax]` (roll-pooled Dmax, median of inliers) and `[gamma]` with `gamma` (absolute at measurement), `ratio`, `offset` (log-D) and `n_patches` (`NeutralFit.patches`, so 2 for one grey card and `N` for `N >= 2`), plus a header `rig` naming the rig profile used (absent when none). At render only `ratio` and `offset` are applied; `[base]`, `[dmax]` and the absolute `gamma` are informational, because Dmin and Dmax depend on the rig and the light. Using a film profile measured with a different rig name than the one in use prints a warning. Profiles are found only by explicit path (`--rig`, `--film`, or `Settings.profiles`); `profiles/` in the repository holds committed ones and there is no search path. This resolves both open points of `docs/PROFILES.md`.
- Gamma source: a frame's Estimates records `GammaSource::Profile` when a film correction supplies the gamma. `NeutralFit` appears only in a film profile's `[gamma]` table (`gamma_source = "neutral_fit"`), written by `negg profile film`.
- Dates: `measured_on` defaults to today's UTC date from `SystemTime` (civil-from-days, no new dependency), overridable with `--measured-on YYYY-MM-DD`; `--source` text is required for both profile commands.

## Consequences

- With one grey patch the base acts as a free second neutral patch, so a single grey card frame fixes both ratios. With two or more patches the base is not added, so the fit is the original model's `N >= 2` system and is not pulled towards the origin by an assumption; two patches are exact and more are least squares.
- The spread floor of 0.1 log-D is a Provisional choice, `TODO(measure)`.
- Taking `gamma_G` from the frame or roll means a film profile never fixes absolute contrast; this keeps the stage independent of the rig and light.
- A film profile with `method = "none"` or no `[gamma]` table gives no film correction; the CLI warns and renders without one.
- No lookup code, search path or environment variable exists to maintain.
- The step 6 cast fit is bypassed when a film correction is in use, so a film profile and auto cast removal are not applied on top of one another.

Cost if wrong: patch, one constant; neutral fit as amended, the threshold for adding the base; contrast, one precedence branch; film profile, lookup code later; dates, a flag change; gamma source, one enum variant.

## Eval delta

None, no render change without a film profile.
