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
| `e_c` | Relative log exposure, `e_c = -d_c / gamma_c` (step 4). Not to be confused with the natural exponent `e^z`. |
| `T_c` | Linear positive, `T_c = 10^(e_c)` in (0, 1] (step 4). |
| `k`, `x_0`, `cmy_c` | Print stage controls: grade (contrast), exposure, and colour timing in log space (step 5). |
| `P_c` | Print density (step 5). |
| `Dmin_p`, `Dmax_p` | Paper white and paper black density (step 5). |
| `out_c` | Scene-linear reflectance after the print stage, `out_c = 10^(-P_c)` (step 5). |
| `softplus(z)` | `softplus(z) = ln(1 + e^z)`. A smooth, monotone approximation to `max(0, z)`. |
| `x <- expr` | In-place update: the quantity on the left is replaced by the expression on the right. |
| grid, cell | The analysis grid of step 3: a block-median downsample of the frame to about 1024 px on the long side. A cell is one element of that grid. |
| log-D | A difference of two densities, used as the unit for tolerances and thresholds. |
| `[a, b]`, `(a, b]` | Closed and half-open intervals. "Clamped to [a, b]" means `min(max(x, a), b)`. |

Per-pixel maths is `f32` from decode to the final encode (ground rule 4). Quantisation happens exactly once, in step 8.

### Pipeline order

The steps are numbered in the order the plan presents them. In the per-pixel pipeline, step 6 (auto colour) runs on the output of step 4 and before step 5, because its correction is applied to the normalised log image. The order of operations on a frame is therefore 1, 2, 3, 4, 6, 5, 7, 8.

### Phase map

| Step | Crate | Phase |
| --- | --- | --- |
| 1 | `negg-decode` | 1 |
| 2 | `negg-calib` | 2 (identity until then) |
| 3 | `negg-film` | 1; roll pooling in phase 2 |
| 4 | `negg-render` (inversion), `negg-film` (gamma estimators) | 1; neutral-patch gamma fit in phase 2 |
| 5 | `negg-render` | 1 |
| 6 | `negg-render` | 1; roll mode in phase 2 |
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

- Record the camera-to-XYZ matrix from `rawler` for use in step 7, but do not apply it here.
- 16-bit TIFF input: honour the embedded ICC profile if it is a matrix/TRC profile (linearise with its TRC); otherwise assume linear if the file says so, else assume gamma 2.2 and emit a warning. Never downconvert to 8-bit.

Constants:

| Name | Default | Notes |
| --- | --- | --- |
| `white_c`, `black_c` | from camera metadata | The documented linear limit and black level, per channel. Never the per-frame maximum. |
| TIFF fallback gamma | 2.2 | Used only when the TIFF carries neither a usable ICC profile nor a linear flag. Always warns. |

Why: the whole model treats `I_c` as proportional to light transmitted through the negative. Any white balance or colour matrix applied here would be baked into the density estimates of step 3 and would make the per-channel gammas of step 4 depend on the camera's rendering choices rather than on the film. Using the documented white level rather than a per-frame maximum keeps a roll on one scale, which is what roll pooling in step 3 relies on. Float32 end to end (ground rule 4) starts here.

## Step 2: rig calibration (phase 2; identity until then)

Phase 2, crate `negg-calib`. Until a rig profile exists every operation in this step is the identity. The three corrections are applied in the order listed, and each is optional on its own: a rig profile may carry any subset. See `docs/PROFILES.md` for how they are stored.

**Flat field.** `F` is a blank-light frame (the light source photographed with no film in the holder), blurred with a Gaussian of sigma equal to the long side of the frame divided by 64.

```
I_c <- I_c * mean(F_c) / blur(F_c)
```

The gain `mean(F_c) / blur(F_c)` is clamped to [0.25, 4].

**Bare-light crosstalk unmix (narrow-band lights).** From three frames, each lit by one LED band, build the 3x3 matrix `M` whose columns are the camera response to each band, with columns normalised so the diagonal is 1. Apply in linear space and clip at 0:

```
I <- M^-1 I
```

**Camera-to-density fit.** Given `N` patches of known density (a transmissive step wedge, or a grey card on film whose densities are estimated from the roll), fit by least squares:

```
D_c = a_c + b_c * log10(I_c) + sum_k c_ck * log10(I_k)
```

This is a 3x3 matrix in log space plus a per-channel offset. Store `a_c`, `b_c` and `c_ck` in the rig profile together with the residual RMS of the fit in log-D.

TODO(spec): the plan says the fit is stored in the rig profile but does not say how it is consumed. The natural reading is that, when a fit is present, it replaces the plain `D_c = -log10(I_c)` of step 3 for that rig. This needs to be pinned down before `negg-calib` lands.

Constants:

| Name | Default | Notes |
| --- | --- | --- |
| flat-field blur sigma | long side / 64 | Gaussian sigma in pixels. |
| flat-field gain clamp | [0.25, 4] | Applied to `mean(F_c) / blur(F_c)`. |
| crosstalk matrix `M` | identity | Columns normalised to unit diagonal; measured per rig. |
| density fit `a_c`, `b_c`, `c_ck` | identity (`a_c = 0`, `b_c = -1`, `c_ck = 0`) | Measured per rig. `TODO(measure)` until a profile exists. |

Why: a camera scanning rig is not a densitometer. Uneven light, filter and sensor crosstalk, and the camera's own spectral response all distort the relationship between pixel value and density. Measuring these once per rig and removing them before any statistics are taken is what lets the same roll scan to the same result under three different light sources (gate 2). Ground rule 6 applies: every number in this step comes from a measurement recorded in a profile with a `source`, never from memory.

## Step 3: density and film base

Phase 1, crate `negg-film`. Roll pooling is phase 2.

Density:

```
D_c = -log10(max(I_c, 1e-6))
```

**Analysis grid.** Downsample the frame to about 1024 px on the long side using block medians (not means), then compute every statistic in this step and in step 6 on the grid, never on the full-resolution image.

**Dmin (film base) per channel.** If the frame has a detectable unexposed border, `Dmin_c` is the median of the flat border cells in the outer 15% band of the grid, where a cell is flat if its std/mean is below 0.03. Otherwise `Dmin_c` is the 0.5th percentile of `D_c` over the grid. The estimator must expose which of the two sources was used.

TODO(spec): the plan does not define what makes a border "detectable", nor the minimum number of flat cells required before the border path is trusted over the percentile path.

**Dmax per channel.** The 99.5th percentile of `D_c` over the grid.

**Roll pooling (phase 2).** For frames in a roll, `Dmin_c` is the per-channel median across frames after rejecting frames whose `Dmin_c` lies more than 0.3 log-D from that median. The same rule pools the per-channel gamma of step 4.

Constants:

| Name | Default | Notes |
| --- | --- | --- |
| `I_c` floor | 1e-6 | Clamp before the log so `D_c` is finite. |
| analysis grid long side | about 1024 px | Block medians, not means. |
| border band | outer 15% | Of the grid, on every edge. |
| flat cell threshold | std/mean < 0.03 | Per cell, on the grid. |
| Dmin percentile | 0.5 | Fallback when no border is detected. |
| Dmax percentile | 99.5 | Always. |
| roll Dmin reject threshold | 0.3 log-D | Distance from the across-frame median. |

Why: ground rule 5. Dust, scratches, sprocket holes and a stray hot pixel sit at the extremes of the density distribution, which is exactly where a naive min/max would look for the film base and Dmax. Block medians on the grid discard isolated outliers before the statistics are taken, and percentiles rather than extremes discard what the medians miss. The phase 1 test is that adding 2% random dust pixels moves the Dmin estimate by less than 0.005 log-D.

## Step 4: inversion to relative log exposure

Phase 1, crate `negg-render` for the inversion and `negg-film` for the gamma estimators. The neutral-patch gamma fit is phase 2.

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

**Neutral-patch fit.** With `N >= 2` patches that should be neutral, with densities above base `d_c,n`, solve for the ratios `gamma_R/gamma_G` and `gamma_B/gamma_G` and the offsets `o_R`, `o_B` by least squares on

```
d_R,n / gamma_R + o_R = d_G,n / gamma_G
d_B,n / gamma_B + o_B = d_G,n / gamma_G
```

Two patches give an exact solution; more give a fit. The ratio is identifiable from the patches; absolute contrast is a user choice.

TODO(spec): the plan fixes the ratios but leaves `gamma_G` itself as "a user choice" and does not say what the default absolute contrast is when a fit is present, nor where the offsets `o_R`, `o_B` are applied (to `e_c`, before or after step 6). Both need a decision before `negg-cli profile film` lands.

Linear positive:

```
T_c = 10^(e_c)
```

with `T_c` in (0, 1].

Constants:

| Name | Default | Notes |
| --- | --- | --- |
| `gamma_c` | `Dmax_c - Dmin_c` | The no-profile fallback. Replaced by a film profile or a neutral fit when one exists. |
| `d_c` floor | 0 | Clamp after subtracting the base. |
| minimum neutral patches `N` | 2 | Exact solution at 2, least squares above. |

Why: a colour negative is three dye layers with different contrasts, and the cast most people see in a naive inversion is the difference between those contrasts, not a white balance error. Dividing each channel by its own gamma removes that difference in the log domain where it is linear. The fallback of using the measured range is the one estimate that needs no stock-specific constant, which is why it is the default under ground rule 6.

## Step 5: print stage (the tone curve)

Phase 1, crate `negg-render`.

Work in print density `P_c`:

```
x_c   = -log10(T_c) * k + x_0 + cmy_c            # k: grade (contrast), x_0: exposure, cmy: colour timing in log space
v_c   = Dmin_p + softplus(a_hl * (x_c - Dmin_p)) / a_hl           # shoulder into paper white
P_c   = Dmax_p - softplus(a_sh * (Dmax_p - v_c)) / a_sh           # toe into paper black
out_c = 10^(-P_c)                                                 # scene-linear reflectance
```

where

```
softplus(z) = ln(1 + e^z)
```

**Grade.** `k` is expressed to the user as an ISO-R style number `R`, default 115, range 50 to 180, with

```
k = 2.9 * range / (R/100)
```

clamped to [2, 10].

TODO(spec): the plan does not define `range` in the grade formula. Until it does, the default `k` for `R = 115` cannot be computed and the formula must not be implemented from a guess.

**Exposure.** `x_0` is chosen so that the frame's exposure anchor prints at `P = 0.75`. The anchor is the trimmed mean of textured grid cells in the midtones.

TODO(spec): the plan does not say how "textured" cells are selected, what density band counts as "midtones", what fraction the trimmed mean trims, or whether the anchor is one value or one per channel. The sign conventions and display units of `x_0` and `cmy_c` as user-facing sliders are also open.

**Optional black point compensation**, applied to `out_c`:

```
out_c <- (out_c - 10^-Dmax_p) / (1 - 10^-Dmax_p)
```

TODO(spec): the plan does not say whether black point compensation is on or off by default.

**Precomputation.** All per-channel coefficients are computed once per render so that the per-pixel kernel is one log, two softplus and one pow.

TODO(spec): the exact set of precomputed coefficients (how `k`, `x_0`, `cmy_c`, `Dmin_p`, `Dmax_p`, `a_hl` and `a_sh` fold together) is left to the implementation. Whatever is chosen must be numerically identical to the four equations above; a property test in `negg-render` should check that.

Constants:

| Name | Default | Notes |
| --- | --- | --- |
| `Dmin_p` | 0.06 | Paper white density. |
| `Dmax_p` | 2.3 | Paper black density. |
| `a_hl` | 3.0 | Shoulder softness; larger is a harder shoulder. |
| `a_sh` | 6.0 | Toe softness; larger is a harder toe. |
| grade `R` | 115 | Range 50 to 180. |
| `k` | `2.9 * range / (R/100)` | Clamped to [2, 10]. `range` is TODO(spec). |
| exposure anchor | `P = 0.75` | `x_0` is solved so the anchor lands here. |
| `cmy_c` | 0 | Colour timing offsets in log space, per channel. |
| black point compensation | optional | Default state is TODO(spec). |

Why: a darkroom print is a second film-like response applied to the positive: a straight line in log space with a soft shoulder into paper white and a soft toe into paper black. Writing it as two softplus terms makes the curve smooth, monotone, and controllable by four numbers that correspond to physical paper properties rather than to an arbitrary spline. Keeping it in `negg-render` as one pure function is ground rule 3; precomputing the coefficients keeps a 24 MP frame under two seconds on a laptop CPU (gate 1).

## Step 6: auto colour (cast removal)

Phase 1, crate `negg-render`. Roll mode is phase 2.

On the normalised log image from step 4 (`e_c` on the analysis grid):

1. Split the cells into three luma bands (shadows, mids, highlights) by the 20th, 50th and 80th percentiles.
2. In each band take the lowest-chroma 30% of cells, excluding cells whose hue falls in the foliage sector (green, 90 to 160 degrees) or the sky sector (blue, 200 to 260 degrees).
3. Fit R against G and B against G as a line through those cells, giving a slope and an offset for each of R and B. Clamp the slope deviation from 1 to plus or minus 0.2.
4. Apply the result as per-channel log-affine corrections to `e_R` and `e_B` before step 5.

TODO(spec): the plan does not define luma, chroma or hue on the normalised log image, nor the exact form of the applied correction (the implied form is to invert the fitted line so that the selected cells land on `e_G`, but this is not written down). Both need a decision before the first render lands.

**Roll mode (phase 2).** Pool the fit across inlier frames by confidence-weighted median.

TODO(spec): the confidence weight and the inlier rule for roll mode are not defined in the plan.

Constants:

| Name | Default | Notes |
| --- | --- | --- |
| luma band percentiles | 20 / 50 / 80 | Boundaries between shadows, mids and highlights. |
| lowest-chroma fraction | 30% | Per band. |
| foliage hue exclusion | 90 to 160 degrees | Green. |
| sky hue exclusion | 200 to 260 degrees | Blue. |
| slope deviation clamp | plus or minus 0.2 | On the fitted slope of R and B against G. |

Why: after step 4 a neutral surface should have `e_R = e_G = e_B`. Whatever residual cast remains is, to first order, an affine error in log space per channel, so fitting a line through the most nearly neutral cells and removing it is the smallest correction that fixes it. Working on three luma bands stops a cast in the shadows from being corrected at the expense of the highlights. Excluding foliage and sky hues stops a frame that is mostly grass or mostly sky from being "corrected" to grey. All of this runs on the block-median grid (ground rule 5), so dust does not pull the fit.

## Step 7: colour management

Phase 1, crate `negg-cms`.

The working space for all post-inversion adjustments is linear Rec.2020 with a D65 white point (decision 2, `docs/ADR/0002-working-space-rec2020.md`). It is entered via the camera matrix recorded in step 1, applied **after** inversion to `out_c`:

```
camera RGB -> XYZ -> Rec.2020
```

with a Bradford chromatic adaptation to D65. Until phase 2's rig fit exists this is the best available characterisation of the camera.

TODO(spec): the plan does not say whether, or how, the step 7 entry changes once a step 2 density fit exists for the rig.

**Export.** Convert via `lcms2` to sRGB, Adobe RGB, Display P3, ProPhoto or Rec.2020, with the ICC profile embedded in the output file.

**Display.** Convert to the monitor profile when it is known, else to sRGB.

Constants:

| Name | Default | Notes |
| --- | --- | --- |
| working space | linear Rec.2020, D65 | Decision 2. Not ProPhoto. |
| chromatic adaptation | Bradford | Camera white to D65. |
| display fallback | sRGB | When no monitor profile is known. |

Why: everything before this step is in camera-native RGB, which has no defined primaries, so no colour-space conversion is possible until inversion is done. Rec.2020 is wide enough to hold any colour a C-41 negative can carry without clipping, and is a standard space with bundled v4 ICC profiles, so the export path is a plain `lcms2` transform rather than something we wrote ourselves.

## Step 8: export

Phase 1, crate `negg-io`. Linear DNG is phase 4.

- 16-bit TIFF: deflate compression, ICC profile in tag 34675, orientation 1, resolution tags present.
- 8-bit JPEG with the ICC profile embedded.
- Linear DNG (phase 4), written with the `tiff` crate. The AGPL `dng` crate is forbidden.
- Sidecar `frame.negg.json` beside the source file containing the settings, the estimated `Dmin_c`, `Dmax_c` and `gamma_c`, the source of each estimate (border, percentile, roll, profile or fit), and a `schema_version`.

This is the only place the pipeline quantises: `f32` is converted to 16-bit or 8-bit integers here and nowhere else (ground rule 4).

TODO(spec): the plan does not fix the resolution tag values, the JPEG quality default or chroma subsampling, or the full sidecar field list beyond the items above.

Constants:

| Name | Default | Notes |
| --- | --- | --- |
| TIFF bit depth | 16 | Deflate, ICC tag 34675, orientation 1. |
| JPEG bit depth | 8 | ICC embedded. Quality is TODO(spec). |
| sidecar | `frame.negg.json` | Beside the source; carries `schema_version`. |

Why: a 16-bit TIFF with the profile embedded preserves everything the `f32` pipeline produced to within the precision a downstream editor can use, and the sidecar makes every render reproducible from the source RAW plus one small file. Recording the source of each estimate in the sidecar is what lets a user, or `negg-cli eval`, tell a border-derived Dmin from a percentile one.

## Keeping this in sync

- Every public function in `negg-render` and `negg-film` has a doc comment linking to the step it implements, using the GitHub heading anchor of that step, for example `docs/MATHS.md#step-4-inversion-to-relative-log-exposure`.
- The anchors are: `#step-1-decode-to-linear-camera-rgb`, `#step-2-rig-calibration-phase-2-identity-until-then`, `#step-3-density-and-film-base`, `#step-4-inversion-to-relative-log-exposure`, `#step-5-print-stage-the-tone-curve`, `#step-6-auto-colour-cast-removal`, `#step-7-colour-management`, `#step-8-export`. Do not rename a step heading without updating every doc comment that links to it.
- Any deviation of the code from this document, including a change to any default in the constants tables, gets an ADR in `docs/ADR/` that records the `negg-cli eval` delta. Never change a default silently.
- Resolving a `TODO(spec)` line is a change to the model: write the decision into this document and into an ADR in the same pull request.
- A `TODO(measure)` is resolved only by a profile with a `source` field; see `docs/PROFILES.md`.
- If the plan and this document disagree, stop and ask; do not pick one.
