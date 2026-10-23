# negg: the model

This document is the mathematical model that `negg` implements. It is transcribed from the eight steps in `IMPLEMENTATION_PLAN.md` and is the single source of truth for the maths: the code follows this document, not the other way round. Every public function in `negg-render` and `negg-film` carries a doc comment linking to one of the step sections below, and any deviation from what is written here needs an architecture decision record in `docs/ADR/` before it is merged.

Where the plan leaves a detail open, this document says so with a `TODO(spec)` line rather than filling the gap. Where a number must come from a measurement that has not yet been made, it says `TODO(measure)`. Nothing here was taken from any other negative conversion software; see ground rule 1 of the plan.

Licence: Apache-2.0 (see `docs/LICENSE`).

## Notation

| Symbol | Meaning |
| --- | --- |
| `c` | A colour channel, `c` in {R, G, B}. Every per-channel quantity carries this subscript. |
| `I_c` | Linear camera RGB in [0, 1] after decode (step 1) and rig calibration (step 2). |
| `log10(x)` | Logarithm base 10. All logs in this document are base 10 unless written `ln`. |
| `ln(x)`, `e^z` | Natural logarithm and natural exponent. They appear only inside `softplus`. |
| `D_c` | Density, `D_c = -log10(I_c)` (step 3). |
| `Dmin_c` | Film base density per channel: the density of unexposed film (step 3). |
| `Dmax_c` | Maximum density per channel (step 3). |
| `d_c` | Density above base, `d_c = D_c - Dmin_c`, clamped at 0 (step 4). |
| `gamma_c` | Layer gamma per channel: the contrast of one dye layer (step 4). |
| `e_c` | Relative log exposure, `e_c = -d_c / gamma_c - offset_c / gamma_G` (step 4; the offset term is zero without a film correction). Not to be confused with the natural exponent `e^z`. |
| `T_c` | Linear positive, `T_c = 10^(e_c)` in (0, 1] (step 4). |
| `k`, `x_0`, `cmy_c` | Print stage controls: grade (contrast), exposure, and colour timing in log space (step 5). |
| `P_c` | Print density (step 5). |
| `Dmin_p`, `Dmax_p` | Paper white and paper black density (step 5). |
| `out_c` | Scene-linear reflectance after the print stage, `out_c = 10^(-P_c)` (step 5). |
| `softplus(z)` | `softplus(z) = ln(1 + e^z)`. A smooth, monotone approximation to `max(0, z)`. |
| `ratio_c`, `offset_c` | Film correction from the neutral-patch fit: `gamma_c / gamma_G` and the log-D offset (step 4). |
| `M`, `G`, `a_c` | Crosstalk matrix and camera-to-density model of the rig (step 2). |
| `x <- expr` | In-place update: the quantity on the left is replaced by the expression on the right. |
| grid, cell | The analysis grid of step 3: a block-median downsample of the frame to about 1024 px on the long side. A cell is one element of that grid. |
| log-D | A difference of two densities, used as the unit for tolerances and thresholds. |
| `[a, b]`, `(a, b]` | Closed and half-open intervals. "Clamped to [a, b]" means `min(max(x, a), b)`. |

Per-pixel maths is `f32` from decode to the final encode (ground rule 4). Quantisation happens exactly once, in step 8.

### Pipeline order

The steps are numbered in the order the plan presents them. In the per-pixel pipeline, step 6 (auto colour) runs on the output of step 4 and before step 5, because its correction is applied to the normalised log image. The order of operations on a frame is therefore 1, 2, 3, 4, 6, 5, 7, 8. The neutral offset of step 4 is applied before step 6, and step 2 runs before the analysis grid of step 3.

### Phase map

| Step | Crate | Phase |
| --- | --- | --- |
| 1 | `negg-decode` | 1 |
| 2 | `negg-calib`, `negg-render` (`apply_rig_linear`) | 2 (identity until then) |
| 3 | `negg-film` | 1; roll pooling in phase 2 (`negg-render` roll driver) |
| 4 | `negg-render` (inversion), `negg-film` (gamma estimators, neutral fit) | 1; neutral-patch gamma fit and film correction in phase 2 |
| 5 | `negg-render` | 1 |
| 6 | `negg-render` | 1; roll mode in phase 2 (cast pooling in `negg-film`) |
| 7 | `negg-cms` | 1 |
| 8 | `negg-io` | 1; linear DNG in phase 4 |

A GPU preview kernel for steps 4 to 7 arrives in phase 4 and is tested against the CPU path on every CI run (ground rule 3).

## Step 1: decode to linear camera RGB

Phase 1, crate `negg-decode`.

Rules:

- Use `rawler` for RAW. Run demosaic and active-area crop only. Do **not** apply white balance, the camera colour matrix, or any sRGB conversion. The output is camera-native RGB at unity gain.
- Scale so that the sensor's calibrated white level maps to 1.0 and black level to 0.0, per channel. Use the camera's documented linear limit, not the per-frame maximum, so every frame of a roll decodes on the same scale. The plan states this rule in words; written as a formula it is:

```
I_c = (raw_c - black_c) / (white_c - black_c)
```

- Record the camera-to-XYZ matrix from `rawler` for use in step 7, but do not apply it here. Record with it the CIE XYZ of the illuminant the matrix was measured under (`Decoded::matrix_white`: D65, D50, D55, D75 or A from the CIE tables; D65 with a warning for any other) (ADR 0012, Proposed).
- 16-bit TIFF input: honour the embedded ICC profile if it is a matrix/TRC profile (linearise with its TRC); otherwise assume linear if the file says so, else assume gamma 2.2 and emit a warning. Never downconvert to 8-bit.
- Demosaic is bilinear for Bayer CFAs, written in `negg-decode`; X-Trans and other CFAs are unsupported in phase 1 and decode returns `Unsupported`. TIFF linearisation (the ICC profile or the gamma 2.2 fallback) is done by the caller through `negg-cms`; the decoder passes the ICC bytes and a transfer flag up, so its output is linear only when that flag says so (ADR 0010, Proposed). An embedded matrix/TRC profile is honoured by undoing its TRC per channel in the profile's own primaries, so steps 3 to 6 see the scan's channels; its colorants give the step 7 matrix. LUT-based profiles are unsupported (ADR 0012, Proposed).

Constants:

| Name | Default | Notes |
| --- | --- | --- |
| `white_c`, `black_c` | from camera metadata | The documented linear limit and black level, per channel. Never the per-frame maximum. |
| TIFF fallback gamma | 2.2 | Used only when the TIFF carries neither a usable ICC profile nor a linear flag. Always warns. |

Why: the whole model treats `I_c` as proportional to light transmitted through the negative. Any white balance or colour matrix applied here would be baked into the density estimates of step 3 and would make the per-channel gammas of step 4 depend on the camera's rendering choices rather than on the film. Using the documented white level rather than a per-frame maximum keeps a roll on one scale, which is what roll pooling in step 3 relies on. Float32 end to end (ground rule 4) starts here.

## Step 2: rig calibration (phase 2; identity until then)

Phase 2, crate `negg-calib` (profile maths) and `negg-render` (`apply_rig_linear`, `density_grid`). Until a rig profile exists every operation in this step is the identity. Each part is optional on its own: a rig profile may carry any subset. See `docs/PROFILES.md` for how they are stored and `docs/ADR/0014-rig-correction-order-and-flat-crosstalk.md` and `docs/ADR/0015-density-fit-and-step-7-with-a-rig.md` (both Proposed) for the rulings below.

**Input frames (R-rig-decode, Ruling 35).** Flat, band and wedge frames are decoded with the same decoder as film frames, with the step 1 scaling, and must be linear (RAW or float TIFF); any other transfer is an error. No flat field or unmix is applied to the flat itself. A rig profile is valid only for the same linear scale as the frames it corrects.

**Order (R-rig-order, Ruling 36).** Step 2 runs on the step 1 output, before the analysis grid of step 3: flat field first, then unmix (clip at 0), then the density model replaces the density formula of step 3 (R-density-use). The caller applies the first two with `negg_render::apply_rig_linear` on its owned image, so the render functions still take a shared reference.

**Flat field (R-flat, Ruling 37).** `F` is a blank-light frame (the light source photographed with no film in the holder). It is reduced to the step 3 analysis grid (`analysis_grid(F, 1024)`, block medians) and blurred there with a separable Gaussian:

```
sigma_cells = (long_side_px / 64) / block
radius      = ceil(3 * sigma_cells)
```

Kernel weights are normalised to sum 1; the edges use symmetric reflection (index -1 maps to 0, -2 to 1, repeated for kernels wider than the grid). `mean(F_c)` is the mean over grid cells of the unblurred block medians. The gain for a cell is clamped to [0.25, 4], and a zero blur gives 4:

```
gain_c = clamp(mean(F_c) / blur(F_c), 0.25, 4)
I_c   <- I_c * gain_c
```

The gain is applied per pixel by bilinear interpolation between cell centres in normalised coordinates, `u = (x + 0.5) / W * gw - 0.5` clamped to [0, gw - 1], and the same for `v` with `H` and the grid height `gh`. The flat must have the frame's exact decoded width and height, otherwise it is an error. The flat is rejected as clipped when more than 1% of grid cells have any channel at or above 0.98.

Blurring on the grid rather than at full resolution is deliberate: a full-resolution blur at sigma about 94 px costs about 40 G operations per frame, and the block median adds under 4 px of extra smoothing against that sigma.

**Bare-light crosstalk unmix (R-crosstalk, Ruling 38).** Three frames are taken, each lit by one LED band (R, G, B alone). For band frame `j` the response `r_j` is the per-channel `patch_median` (R-patch, step 4) of its flat-corrected grid over the central rect `{x 0.25, y 0.25, w 0.5, h 0.5}`. Column `j` of `M` is `r_j / r_j[j]`, so the diagonal is 1. It is an error when `r_j[j]` is not the largest channel of `r_j` (frames swapped or not narrow band) or when `|det M| < 1e-6`. The profile stores `M`; the runtime stores `M^-1`. Apply in linear space and clip at 0:

```
I <- max(M^-1 I, 0)
```

**Camera-to-density fit (R-density-fit, Ruling 39).** Given `N` patches of known density from a transmissive step wedge, the stored model is

```
D_c = a_c + sum_k G_ck * log10(max(I_k, 1e-6))
```

with `b_c = G_cc` (own-channel slope), `c_ck = G_ck` for `k != c` (cross terms) and `c_cc = 0`: the `b_c` and `c_cc` of the plan's formula multiply the same regressor and are not separately identifiable on the diagonal. The patch value `I` is `patch_median` of the rig-corrected (flat, unmix) wedge grid and the known `D` comes from the wedge CSV (see `docs/PROFILES.md`). Fit each channel by least squares (singular value decomposition):

- Full model when `N >= 8` and the condition number of the centred `N x 3` design matrix of `log10 I` is at most 100.
- Otherwise the diagonal model (`c = 0`, fitting `a_c` and `b_c`) when `N >= 3`, with `model = "diagonal"` and a warning.
- Otherwise an error. It is also an error when any `b_c >= 0`.

`residual_rms = sqrt(mean of the 3N squared residuals)`, in log-D. The condition number decides the model and appears in the diagonal-fallback warning (with the reason: fewer than 8 patches, a condition number above 100, or an infinite one), but it is not stored: the rig profile schema has no field for it. A neutral silver wedge makes the three log channels collinear, so the cross terms are unidentifiable from it; only coloured patches (per-channel densities) can fit them, which is why the diagonal fallback exists.

**Use of the fit (R-density-use, Ruling 40).** With a density model, the density of step 3 is the model, both on the analysis grid (Dmin, Dmax, cast, anchor) and per pixel in the full-resolution render; without one it is `-log10(max(I, 1e-6))`. The identity model is `a = 0`, `G = -I`. `a_c` cancels in the Dmin subtraction of step 4 but is kept so that sidecar densities are wedge-referenced. The source of a fit from a grey card on film, with densities estimated from the roll, is deferred; phase 2 fits from a wedge only.

**Step 7 is unchanged (R-step7, Ruling 41).** A rig profile does not change step 7; see there.

Constants:

| Name | Default | Notes |
| --- | --- | --- |
| flat-field blur sigma | long side / 64 | In pixels, realised on the grid as `sigma_cells = (long_side_px / 64) / block`. |
| flat-field gain clamp | [0.25, 4] | Applied to `mean(F_c) / blur(F_c)`. |
| flat-field kernel radius | `ceil(3 * sigma_cells)` | Weights normalised to sum 1, symmetric reflection. |
| flat clipped-cell level | 0.98, more than 1% of cells | Any channel at or above the level marks a cell. Controller choice, `TODO(measure)`. |
| crosstalk response rect | `{0.25, 0.25, 0.5, 0.5}` | Central rect of each band frame. Controller choice, `TODO(measure)`. |
| crosstalk determinant floor | `1e-6` | Error below this. Controller choice, `TODO(measure)`. |
| crosstalk matrix `M` | identity | Columns normalised to unit diagonal; measured per rig. |
| density fit full model | `N >= 8`, condition number at most 100 | Else the diagonal model. Controller choice, `TODO(measure)`. |
| density fit diagonal model | `N >= 3` | Else error. Controller choice, `TODO(measure)`. |
| density fit `a_c`, `G_ck` | identity (`a_c = 0`, `G = -I`) | Measured per rig. `TODO(measure)` until a profile exists. |
| density model floor | `1e-6` | Clamp before the log, as in step 3. |

Why: a camera scanning rig is not a densitometer. Uneven light, filter and sensor crosstalk, and the camera's own spectral response all distort the relationship between pixel value and density. Measuring these once per rig and removing them before any statistics are taken is what lets the same roll scan to the same result under three different light sources (gate 2). Ground rule 6 applies: every number in this step comes from a measurement recorded in a profile with a `source`, never from memory.

## Step 3: density and film base

Phase 1, crate `negg-film`. Roll pooling is phase 2 (`negg-film` pooling functions, `negg-render` roll driver).

With a rig density model (step 2, R-density-use) the density below is the model; the formula shown is the default without one.

Density:

```
D_c = -log10(max(I_c, 1e-6))
```

**Analysis grid.** Downsample the frame to about 1024 px on the long side using block medians (not means), then compute every statistic in this step and in step 6 on the grid, never on the full-resolution image.

**Dmin (film base) per channel.** If the frame has a detectable unexposed border, `Dmin_c` is the median of the flat border cells in the outer 15% band of the grid, where a cell is flat if its std/mean is below 0.03. Otherwise `Dmin_c` is the 0.5th percentile of `D_c` over the grid. The estimator must expose which of the two sources was used.

The border is detectable, and the border path used, only when at least 20 cells and at least 5% of the cells in the outer 15% band are flat and the per-channel median density of those flat cells is at most 0.05 log-D above the 0.5th-percentile density of the whole grid; otherwise the percentile path is used, and the estimator reports which path was taken (ADR 0007, Proposed).

**Dmax per channel.** The 99.5th percentile of `D_c` over the grid.

**Roll pooling (phase 2).** A roll is the set of batch inputs of one directory (R-roll, Ruling 47; ADR 0017, Proposed). For each pooled quantity every frame has a role: Inlier, Rejected, Excluded (the user fixed that quantity in the frame's resolved settings) or Unused (not a candidate). R-pool (Ruling 48):

- **Dmin candidates:** the frames whose `dmin_source` is Border when at least 2 non-excluded frames are Border, else all non-excluded frames.
- **Pool:** take the per-channel median over the candidates. Reject a frame when any channel lies more than 0.3 log-D from that median (Chebyshev distance). The pooled value is the per-channel median of the inliers. No inliers means no pooled value: the frames keep their own estimates and a warning is issued.
- **Gamma:** per frame `gamma_f = max(Dmax_f - Dmin_pooled, 0.05)` per channel (the frame's own Dmin if none is pooled), pooled by the same rule over all non-excluded frames. This is the step 4 fallback applied to the pooled base. The pooled gamma G also supplies `gamma_G` for the neutral-patch fit and the film correction in step 4.
- **Inlier frames for Gate 2** are the Dmin inliers.

The cast pool is defined in step 6. Precedence per estimate: settings Fixed, then film correction (gamma and offsets), then the roll pool, then the frame's own estimate (R-lock, Ruling 50). A frame whose quantity is Fixed (role Excluded) neither feeds nor takes that pool. A frame Rejected from a pool does take the pooled value when it is rendered (Ruling 59): the Rejected role only keeps it out of the median, so an outlier is rendered with the roll's value, and a user who wants it to keep its own estimate fixes it with `negg lock`. When no pooled value exists (no inliers), every frame keeps its own estimate (Ruling 48).

Constants:

| Name | Default | Notes |
| --- | --- | --- |
| `I_c` floor | 1e-6 | Clamp before the log so `D_c` is finite. |
| analysis grid long side | about 1024 px | Block medians, not means. |
| border band | outer 15% | Of the grid, on every edge. |
| flat cell threshold | std/mean < 0.03 | Per cell, on the grid. |
| Dmin percentile | 0.5 | Fallback when no border is detected. |
| border minimum flat cells | 20 | Needed before the border path is trusted. Controller choice, `TODO(measure)` (ADR 0007, Proposed). |
| border minimum flat fraction | 5% | Of the cells in the outer 15% band, also needed. Controller choice, `TODO(measure)` (ADR 0007, Proposed). |
| border Dmin tolerance | 0.05 log-D | Flat-cell median at most this far above the grid's 0.5th percentile, per channel. Controller choice, `TODO(measure)` (ADR 0007, Proposed). |
| Dmax percentile | 99.5 | Always. |
| roll reject threshold | 0.3 log-D | Chebyshev distance of a frame's per-channel value from the across-frame median, for Dmin and gamma. |
| roll Border preference | at least 2 non-excluded Border frames | Else all non-excluded frames are Dmin candidates. Controller choice, `TODO(measure)` (ADR 0017, Proposed). |
| pooled gamma floor | 0.05 log-D | `gamma_f = max(Dmax_f - Dmin_pooled, 0.05)`. Same value as the step 4 floor. |

Why: ground rule 5. Dust, scratches, sprocket holes and a stray hot pixel sit at the extremes of the density distribution, which is exactly where a naive min/max would look for the film base and Dmax. Block medians on the grid discard isolated outliers before the statistics are taken, and percentiles rather than extremes discard what the medians miss. The phase 1 test is that adding 2% random dust pixels moves the Dmin estimate by less than 0.005 log-D.

## Step 4: inversion to relative log exposure

Phase 1, crate `negg-render` for the inversion and `negg-film` for the gamma estimators. The neutral-patch gamma fit and the film correction are phase 2.

Density above base, clamped at 0:

```
d_c = D_c - Dmin_c
```

Per-channel contrast normalisation, where `gamma_c` is the layer gamma:

```
e_c = -d_c / gamma_c
```

**No-profile fallback.** The default layer gamma is the measured density range of the frame (or of the roll, with pooling):

```
gamma_c = Dmax_c - Dmin_c
```

so that each channel spans [0, -1]. This is equivalent to an independent per-channel stretch and is what every frame gets when no film profile and no neutral samples are available (ground rule 6).

**Patch statistic (R-patch, Ruling 43).** `patch_median(grid_image, rect)` is the per-channel median over the grid cells whose centres lie inside `rect` shrunk by 10% of its width and height on each side. It returns nothing when fewer than 4 cells qualify. It is the one patch statistic used for wedge steps (step 2), band responses (step 2) and neutral patches (below).

**Neutral-patch fit (R-neutral-fit, Ruling 44).** The inputs are densities above base `d_n = max(D_n - Dmin, 0)` for `N >= 1` grey patches, where `D_n` is `patch_median` of the grid density and `Dmin` is the roll-pooled value. Only when exactly one grey patch is given (`N = 1`) is the film base itself added as a second patch `d = (0, 0, 0)`: unexposed film is neutral by construction of the Dmin subtraction. With `N >= 2` the patches alone are fitted (Ruling 58, which amends Ruling 44). Given `gamma_G` (below), for `c` in {R, B} take the ordinary least squares fit of `y_n = d_G,n / gamma_G` on `x_n = d_c,n`:

```
y_n = s_c * x_n + o_c        s_c = 1 / gamma_c        o_c = the offset o_c of the plan
ratio_c  = gamma_c / gamma_G = 1 / (s_c * gamma_G)
offset_c = o_c * gamma_G                        (log-D; G entries are ratio 1, offset 0)
```

This is the plan's system `d_c,n / gamma_c + o_c = d_G,n / gamma_G`. Two points fix the line, so one grey patch (with the base added) and two grey patches (alone) both give an exact solution; three or more patches give a least-squares fit. Let `P` be the number of points actually used: `P = 2` when `N = 1`, else `P = N`. It is an error when the largest `d_G` over the points used is below 0.1 log-D (no spread to fit) or when a slope is not positive. The fit reports `residual_rms = gamma_G * RMS` of the `2P` e-space residuals, in log-D, and the patch count `P` (`NeutralFit.patches`), which is what a film profile stores as `n_patches`. So one grey card frame suffices.

**Absolute contrast and where the offsets apply (R-contrast, Ruling 45).** Absolute contrast is the green range: `gamma_G = Dmax_G - Dmin_G` of the frame, or the roll-pooled `gamma_G` in roll mode (step 3). Then `gamma_c = ratio_c * gamma_G`. The offsets apply as

```
e_c = -d_c / gamma_c - offset_c / gamma_G
```

before step 6. The offsets are in log-D and are always divided by `gamma_G`, the green gamma in effect for the frame, including when that gamma is Fixed (a lock or override) or pooled. `Estimates::neutral_offset` stores the e-unit value `offset_c / gamma_G` (G is 0), and the render subtracts it from `e_c`, so a positive stored offset lowers `e_c`. When a film correction is in use, the step 6 cast fit is skipped (identity, `cast_source = Identity`), because the measured neutral fit already removes the same log-affine error.

**Precedence (R-lock, Ruling 50).** For Dmin, gamma and cast, in order: a settings value that is Fixed (a lock or override), then the film correction (gamma and offsets), then the roll pool, then the frame's own estimate. The `gamma_source` variants are Range, Fixed, NeutralFit, Roll and Profile. A frame's Estimates records `GammaSource::Profile` whenever a film correction supplies the gamma (Ruling 57); NeutralFit never appears in a frame's Estimates and is only the value `negg profile film` writes in a film profile's `[gamma]` table (`gamma_source = "neutral_fit"`). Dmax is Fixed or estimated.

**Film profile (R-film-profile, Ruling 46).** A film profile (`docs/PROFILES.md`) stores `ratio`, `offset` and the absolute `gamma` at measurement. At render only `ratio` and `offset` are applied, with `gamma_G` taken from the frame or roll as above. A film profile's `[base]`, `[dmax]` and absolute `gamma` are informational, because Dmin and Dmax depend on the rig and the light. Profiles are found only by explicit path (ADR 0016, Proposed). A film profile with `method = "none"` or without a `[gamma]` table gives no film correction: the CLI warns and renders without one, using the no-profile fallback above. With a film correction no cast is pooled and the cast is the identity (step 6).

Linear positive:

```
T_c = 10^(e_c)
```

with `T_c` in (0, 1].

Constants:

| Name | Default | Notes |
| --- | --- | --- |
| `gamma_c` | `Dmax_c - Dmin_c` | The no-profile fallback. Replaced by `ratio_c * gamma_G` when a film correction exists. |
| `d_c` floor | 0 | Clamp after subtracting the base. |
| `gamma_c` floor | 0.05 log-D | Per channel, on the no-profile fallback `Dmax_c - Dmin_c`, so that step 4 never divides by zero or a near-zero range on a flat frame. Controller choice, `TODO(measure)`. |
| minimum neutral patches `N` | 1 grey patch | With `N = 1` the base is added as the second point; with `N >= 2` it is not (R-neutral-fit, Ruling 58). |
| neutral patch shrink | 10% of the rect on each side | Applied before selecting cells. Controller choice, `TODO(measure)` (ADR 0016, Proposed). |
| neutral patch minimum cells | 4 | Fewer cells and `patch_median` returns nothing. Controller choice, `TODO(measure)` (ADR 0016, Proposed). |
| neutral fit minimum spread | 0.1 log-D | Largest `d_G` over the patches. Controller choice, `TODO(measure)` (ADR 0016, Proposed). |
| absolute contrast `gamma_G` | `Dmax_G - Dmin_G` (frame or pooled) | `gamma_c = ratio_c * gamma_G` with a film correction (R-contrast). |
| neutral offset | `e_c -= offset_c / gamma_G` | Subtracted from `e_c` before step 6, after the `-d_c / gamma_c` term; `gamma_G` is the green gamma in effect, also when Fixed. Zero without a film correction. |

Why: a colour negative is three dye layers with different contrasts. Dividing each channel by its own gamma removes that difference in the log domain, where it is linear, which is what a single white balance cannot do. The fallback of using the measured range is the one estimate that needs no stock-specific constant, which is why it is the default under ground rule 6.

## Step 5: print stage (the tone curve)

Phase 1, crate `negg-render`.

Work in print density `P_c`:

```
x_c   = log10(T_c) * k + x_0 + cmy_c             # k: grade (contrast), x_0: exposure, cmy: colour timing in log space
v_c   = Dmin_p + softplus(a_hl * (x_c - Dmin_p)) / a_hl           # shoulder into paper white
P_c   = Dmax_p - softplus(a_sh * (Dmax_p - v_c)) / a_sh           # toe into paper black
out_c = 10^(-P_c)                                                 # scene-linear reflectance
```

The sign differs from the plan's draft; see ADR 0011 (Proposed).

where

```
softplus(z) = ln(1 + e^z)
```

**Grade.** `k` is expressed to the user as an ISO-R style number `R`, default 115, range 50 to 180, with

```
k = 2.9 * range / (R/100)
```

clamped to [2, 10].

`range` is the normalised log-exposure span of the inverted negative from step 4, which is 1.0 by construction (each channel spans [0, -1]), so `k = 290 / R`: `R = 115` gives `k = 2.52`, `R = 50` gives 5.8 and `R = 180` gives 2.0 after the clamp (ADR 0006, Proposed).

**Exposure.** `x_0` is chosen so that the frame's exposure anchor prints at `P = 0.75`. The anchor is the trimmed mean of textured grid cells in the midtones.

The anchor `a` is one scalar: on the grid after step 4, luma is `l = (e_R + e_G + e_B) / 3`, a cell is textured when its block's std/mean is at least 0.03, midtones are the textured cells whose `l` lies between the 25th and 75th percentiles of `l` over textured cells, and `a` is the 10% trimmed mean of `l` over those cells (the median of `l` over all grid cells if fewer than 16 midtone cells exist). `x_0` is solved analytically so that `x = a * k + x_0` prints at `P = 0.75`, using `softplus_inv(y) = ln(e^y - 1)`: `v = Dmax_p - softplus_inv(a_sh * (Dmax_p - 0.75)) / a_sh`, `x_target = Dmin_p + softplus_inv(a_hl * (v - Dmin_p)) / a_hl`, `x_0 = x_target - a * k`; the user's `exposure` is added to `x_0` and `cmy_c` to `x_c`, positive meaning more print density (darker) in both cases (ADR 0008, Proposed).

**Optional black point compensation**, applied to `out_c`:

```
out_c <- (out_c - 10^-Dmax_p) / (1 - 10^-Dmax_p)
```

Default off (reading of the plan's "optional"; see `Settings` in `negg-types`).

**Precomputation.** All per-channel coefficients are computed once per render so that the per-pixel kernel is one log, two softplus and one pow.

The coefficients are fixed by `PrintCoefficients` in `negg-render`: the grade `k`, the per-channel total `x_0 + exposure + cmy_c` (so `x_c = log10(T_c) * k + (x_0 + exposure + cmy_c)`), `Dmin_p`, `Dmax_p`, `a_hl`, `a_sh`, and for black point compensation the floor `10^-Dmax_p` and its complement `1 - 10^-Dmax_p`. Nothing else is folded. A property test in `negg-render` checks the kernel against the four equations written out in f64 with no precomputation.

Constants:

| Name | Default | Notes |
| --- | --- | --- |
| `Dmin_p` | 0.06 | Paper white density. |
| `Dmax_p` | 2.3 | Paper black density. |
| `a_hl` | 3.0 | Shoulder softness; larger is a harder shoulder. |
| `a_sh` | 6.0 | Toe softness; larger is a harder toe. |
| grade `R` | 115 | Range 50 to 180. |
| `k` | `290 / R` | Clamped to [2, 10]. `range` in `2.9 * range / (R/100)` is 1.0 by construction; the 2.9 has no cited source, `TODO(measure)` (ADR 0006, Proposed). |
| exposure anchor | `P = 0.75` | `x_0` is solved so the anchor lands here. |
| anchor textured threshold | std/mean >= 0.03 | Complement of the step 3 flat threshold (ADR 0008, Proposed). |
| anchor midtone band | 25th to 75th percentile | Of luma over textured cells. Controller choice, `TODO(measure)` (ADR 0008, Proposed). |
| anchor trim | 10% | Dropped from each end of the midtone luma values. Controller choice, `TODO(measure)` (ADR 0008, Proposed). |
| anchor minimum midtone cells | 16 | Below this the anchor is the median luma of all grid cells. Controller choice, `TODO(measure)` (ADR 0008, Proposed). |
| `cmy_c` | 0 | Colour timing offsets in log space, per channel. |
| black point compensation | off | Optional; default off per the plan's "optional" and `Settings` in `negg-types`. |

Why: a darkroom print is a second film-like response applied to the positive: a straight line in log space with a soft shoulder into paper white and a soft toe into paper black. Writing it as two softplus terms makes the curve smooth, monotone, and controllable by four numbers that correspond to physical paper properties rather than to an arbitrary spline. Keeping it in `negg-render` as one pure function is ground rule 3; precomputing the coefficients keeps a 24 MP frame under two seconds on a laptop CPU (gate 1).

## Step 6: auto colour (cast removal)

Phase 1, crate `negg-render`. Roll mode is phase 2.

On the normalised log image from step 4 (`e_c` on the analysis grid):

1. Split the cells into three luma bands (shadows, mids, highlights) by the 20th, 50th and 80th percentiles.
2. In each band take the lowest-chroma 30% of cells, excluding cells whose hue falls in the foliage sector (green, 90 to 160 degrees) or the sky sector (blue, 200 to 260 degrees).
3. Fit R against G and B against G as a line through those cells, giving a slope and an offset for each of R and B. Clamp the slope deviation from 1 to plus or minus 0.2.
4. Apply the result as per-channel log-affine corrections to `e_R` and `e_B` before step 5.

Luma and hue are read in print-space orientation, on `p = -e` per grid cell, because after ADR 0011 a more negative `e_c` prints brighter in channel `c`: a cell that prints green has `p_G` largest, and the shadows are the cells that print darkest (ADR 0009, amended by Ruling 30). Luma is `l = (p_R + p_G + p_B) / 3`, chroma is `c = max(p) - min(p)` (equal to `max(e) - min(e)`), hue is the standard HSV hue angle of `p` (undefined when `c < 1e-4`, in which case the cell cannot fall in an excluded sector and is kept), the bands are `[p0, p20)`, `[p20, p80)` and `[p80, p100]` of `l` with the 50th percentile as the centre of the mids band rather than a boundary, and the hue exclusion is applied before the 30% chroma cut. The kept cells of all three bands are pooled and fitted by ordinary least squares as `e_R = s_R * e_G + o_R` and `e_B = s_B * e_G + o_B` with the slopes clamped to [0.8, 1.2] (when a slope is clamped the offset is recomputed through the centroid of the pooled cells, `o = mean(e_target) - s_clamped * mean(e_G)`, so the selected cells still land on `e_G` on average (ADR 0009, amended)), and the correction is applied to the full-resolution image as `e_R <- (e_R - o_R) / s_R` and `e_B <- (e_B - o_B) / s_B` so the selected cells land on `e_G`; if fewer than 8 cells survive, the correction is the identity and the estimator says so (ADR 0009, Proposed).

**Roll mode (phase 2).** Each non-excluded frame's cast is fitted on its own analysis grid inverted with the pooled Dmin and gamma (and the film correction offsets, if any), and the fits are pooled by confidence-weighted median (R-cast-pool, Ruling 49; ADR 0017, Proposed):

- **Candidates:** frames not Rejected for Dmin or gamma in step 3, with at least one cell used in the fit. A frame with `colour.cast` fixed, or with `auto_colour` off, is Excluded.
- **Confidence weight:** the number of cells used in the frame's fit, `cells_used`.
- **Pooled value:** each of `slope_r`, `offset_r`, `slope_b` and `offset_b` is the weighted median of its own values: sort ascending and take the first value at which the cumulative weight reaches half the total weight. The pooled `cells_used` is the sum of the candidate weights. No candidates means no pooled cast.

A pooled cast is used by frames that are not fixed, in place of their own fit, including frames Rejected from the pool (Ruling 59). The precedence is in step 3. With a film correction in use the cast is the identity (R-contrast), nothing is pooled and every non-fixed frame has `cast_source = Identity`. The `cast_source` of each frame records which of Fit, Fixed, Roll or Identity applied (R-cast-source, Ruling 55; ADR 0018, Proposed): Identity when auto colour is off, fewer than 8 cells survive, or a film correction is in use.

Constants:

| Name | Default | Notes |
| --- | --- | --- |
| luma band percentiles | 20 / 50 / 80 | Of luma on `-e`. 20 and 80 are the band boundaries; 50 marks the centre of the mids band (ADR 0009, Proposed). |
| lowest-chroma fraction | 30% | Per band. |
| foliage hue exclusion | 90 to 160 degrees | Green in the print: hue of `-e` (ADR 0009, amended). |
| sky hue exclusion | 200 to 260 degrees | Blue in the print: hue of `-e` (ADR 0009, amended). |
| slope deviation clamp | plus or minus 0.2 | On the fitted slope of R and B against G. |
| chroma floor | 1e-4 | Below this hue is undefined and the cell is kept. Controller choice, `TODO(measure)` (ADR 0009, Proposed). |
| minimum fit cells | 8 | Below this the correction is the identity. Controller choice, `TODO(measure)` (ADR 0009, Proposed). |
| auto colour | on | Default lives in `ColourSettings` in `negg-types`. |
| roll cast weight | `cells_used` | Weight of each frame in the weighted median (R-cast-pool). Controller choice, `TODO(measure)` (ADR 0017, Proposed). |
| fixed cast slopes | finite, in [0.5, 2] | `colour.cast` Fixed values; offsets finite (validation in `negg-types`). |

Why: after step 4 a neutral surface should have `e_R = e_G = e_B`. Whatever residual cast remains is, to first order, an affine error in log space per channel, so fitting a line through the most nearly neutral cells and removing it is the smallest correction that fixes it. Working on three luma bands stops a cast in the shadows from being corrected at the expense of the highlights. Excluding foliage and sky hues stops a frame that is mostly grass or mostly sky from being "corrected" to grey. All of this runs on the block-median grid (ground rule 5), so dust does not pull the fit.

## Step 7: colour management

Phase 1, crate `negg-cms`.

The working space for all post-inversion adjustments is linear Rec.2020 with a D65 white point (decision 2, `docs/ADR/0002-working-space-rec2020.md`). It is entered via the camera matrix recorded in step 1, applied **after** inversion to `out_c`:

```
camera RGB -> XYZ -> Rec.2020
```

with a Bradford chromatic adaptation to D65. The phase 2 rig fit does not replace it (R-step7, below); this is the best available characterisation of the camera.

The matrix is white-normalised, so that a camera neutral (equal channels after steps 4 to 6) lands on a Rec.2020 neutral. With `X2C` the recorded XYZ-to-camera matrix and `W` the XYZ of its illuminant:

```
M = XYZ_TO_REC2020 * Bradford(W -> D65) * inverse(X2C) * diag(X2C * W)
```

so camera `(1, 1, 1)` maps to `W`, then to D65, then to Rec.2020 `(1, 1, 1)`. For a TIFF linearised through a matrix/TRC ICC profile in its own primaries (step 1), the entry is `M = XYZ_TO_REC2020 * Bradford(D50 -> D65) * C`, with `C` the profile's colorant matrix (columns rXYZ, gXYZ, bXYZ, relative to the D50 profile connection space), which is white-normalised by construction (ADR 0012, Proposed).

**Unchanged by a rig profile (R-step7, Ruling 41; ADR 0015, Proposed).** The white-normalised matrix above is applied to `out_c` whatever the rig: flat field, unmix and the density model do not alter it. No phase 2 measurement characterises the colorimetry of the positive, a wedge constrains only the grey axis, and white normalisation already maps neutrals to neutrals, so neutrality does not depend on this choice. Until a measured characterisation exists, saturation and hue come from the camera matrix alone.

**Export.** Convert via `lcms2` to sRGB, Adobe RGB, Display P3, ProPhoto or Rec.2020, with the ICC profile embedded in the output file.

**Display.** Convert to the monitor profile when it is known, else to sRGB.

Constants:

| Name | Default | Notes |
| --- | --- | --- |
| working space | linear Rec.2020, D65 | Decision 2. Not ProPhoto. |
| chromatic adaptation | Bradford | Matrix illuminant white (or the ICC D50) to D65. |
| matrix white normalisation | `diag(X2C * W)` | Camera `(1, 1, 1)` is the illuminant white (ADR 0012, Proposed). |
| display fallback | sRGB | When no monitor profile is known. |
| output space | sRGB | Default lives in `ColourSettings` in `negg-types`. |

Why: everything before this step is in camera-native RGB, which has no defined primaries, so no colour-space conversion is possible until inversion is done. Rec.2020 is a standard wide space with bundled v4 ICC profiles (decision 2), so the export path is a plain `lcms2` transform rather than something we wrote ourselves.

## Step 8: export

Phase 1, crate `negg-io`. Linear DNG is phase 4.

- 16-bit TIFF: deflate compression, ICC profile in tag 34675, orientation 1, resolution tags present.
- 8-bit JPEG with the ICC profile embedded.
- Linear DNG (phase 4), written with the `tiff` crate. The AGPL `dng` crate is forbidden.
- Sidecar `frame.negg.json` beside the source file containing the settings, the estimated `Dmin_c`, `Dmax_c` and `gamma_c`, the source of each estimate (`dmin_source`: border, percentile, fixed, roll or profile; `gamma_source`: range, fixed, neutral_fit, roll or profile; `cast_source`: fit, fixed, roll or identity), and a `schema_version`.

This is the only place the pipeline quantises: `f32` is converted to 16-bit or 8-bit integers here and nowhere else (ground rule 4).

TIFF resolution tags are 300 pixels per inch with `ResolutionUnit` inch; JPEG quality is 92 with the encoder's default chroma subsampling; the sidecar fields are `schema_version`, `source` (file name only), `settings`, `settings_hash`, `estimates` (optional: `dmin`, `dmin_source`, `dmax`, `gamma`, `gamma_source`, `neutral_offset`, `anchor`, `x0`, `cast`, `cast_source`), `overrides` (phase 2: the frame's `FrameOverrides`, omitted when empty), `roll` (phase 2: `roll.negg.json` when rendered in roll mode) and `negg_version` (ADR 0010, Proposed; schema version 2 per ADR 0018, Proposed). Version 1 sidecars migrate with `neutral_offset = [0, 0, 0]` and a `cast_source` of Fit when `cast.cells_used > 0`, else Identity. A roll sidecar `roll.negg.json` (its own schema version 1) beside the frames holds the shared `RollSettings`, the pool and the frame list (ADR 0017, Proposed).

Constants:

| Name | Default | Notes |
| --- | --- | --- |
| TIFF bit depth | 16 | Deflate, ICC tag 34675, orientation 1. |
| TIFF resolution | 300 ppi | `XResolution`, `YResolution`, `ResolutionUnit` inch (ADR 0010, Proposed). |
| JPEG bit depth | 8 | ICC embedded. Quality 92, encoder default chroma subsampling (ADR 0010, Proposed). |
| sidecar | `frame.negg.json` | Beside the source; carries `schema_version`. |

Why: a 16-bit TIFF with the profile embedded preserves everything the `f32` pipeline produced to within the precision a downstream editor can use, and the sidecar makes every render reproducible from the source RAW plus one small file. Recording the source of each estimate in the sidecar is what lets a user, or `negg-cli eval`, tell a border-derived Dmin from a percentile one.

## Keeping this in sync

- Every public function in `negg-render` and `negg-film` has a doc comment linking to the step it implements, using the GitHub heading anchor of that step, for example `docs/MATHS.md#step-4-inversion-to-relative-log-exposure`.
- The anchors are: `#step-1-decode-to-linear-camera-rgb`, `#step-2-rig-calibration-phase-2-identity-until-then`, `#step-3-density-and-film-base`, `#step-4-inversion-to-relative-log-exposure`, `#step-5-print-stage-the-tone-curve`, `#step-6-auto-colour-cast-removal`, `#step-7-colour-management`, `#step-8-export`. Do not rename a step heading without updating every doc comment that links to it.
- Any deviation of the code from this document, including a change to any default in the constants tables, gets an ADR in `docs/ADR/` that records the `negg-cli eval` delta. Never change a default silently.
- Resolving a `TODO(spec)` line is a change to the model: write the decision into this document and into an ADR in the same pull request. The one recorded exception is the step 5 coefficients line (Ruling 61): it documents what phase 1 code already does (`PrintCoefficients`, decided under ADR 0011) and changes no model, so it needed no new ADR.
- A `TODO(measure)` is resolved only by a profile with a `source` field; see `docs/PROFILES.md`.
- If the plan and this document disagree, stop and ask; do not pick one.
