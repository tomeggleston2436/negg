# negg: rig and film profiles

This document is the schema for the two profile files `negg` reads: a rig profile (`<name>.rig.negg.toml`) describing the scanning setup, and a film profile (`<name>.film.negg.toml`) describing a film stock. Both use the `*.negg.toml` suffix fixed by decision 4 (`docs/ADR/0004-naming.md`). The maths each table feeds is in `docs/MATHS.md`; this document only says how the numbers are stored and where they came from.

Licence: Apache-2.0 (see `docs/LICENSE`). Profile data committed to this repository is also Apache-2.0 (decision 1).

## Provenance rules

These rules follow ground rule 6 of the plan ("measured, not guessed") and are not negotiable.

1. Every numeric value in a profile needs a `source`: free text saying how it was measured, in the table that holds the value or in the file header when the whole file comes from one measurement. A value with no traceable source does not get committed.
2. No stock-specific constants may be committed without a measurement. There is no "typical Portra 400 Dmin" in this repository. If you cannot cite a measurement, do not ship the number: put a `TODO(measure)` comment and leave the table out so the fallback applies.
3. A missing profile, a missing table, or a `method = "none"` film profile means the no-profile fallback in `docs/MATHS.md#step-4-inversion-to-relative-log-exposure`: `gamma_c = Dmax_c - Dmin_c` with `Dmin_c` and `Dmax_c` estimated from the frame or roll per step 3. A missing rig profile means every step 2 correction is the identity.
4. `residual_rms` is reported by the tool that made the fit, in log-D, and is never typed by hand.
5. Profiles are data, not code. Changing a committed profile is a measurement, not a tuning; the commit must say what was re-measured and the `negg-cli eval` delta goes in the pull request like any engine change (ground rule 7).

## Common header

Both profile kinds start with the same top-level fields.

| Field | Type | Meaning |
| --- | --- | --- |
| `schema_version` | integer | Always `1` for this schema. Readers reject other values. |
| `name` | string | Human-readable name of the rig or stock. |
| `source` | string | Free text: how the profile was measured (equipment, target, procedure). |
| `measured_on` | string | ISO 8601 date, `YYYY-MM-DD`. |
| `method` | string, or array of strings for a rig | Which procedure produced the profile; enum below. |
| `residual_rms` | float | Residual RMS of the governing fit, in log-D. `0.0` only with a `TODO(measure)` comment. |

`method` enum:

| Profile | Values | Meaning |
| --- | --- | --- |
| rig | `"flat_field"`, `"crosstalk"`, `"density_fit"` | One per sub-table present. Each sub-table carries its own `method` with the matching value; the top-level `method` is the array of sub-table methods present, for example `["flat_field", "density_fit"]`. |
| film | `"grey_card_roll"` | Derived from a roll containing a grey card frame by `negg-cli profile film` (neutral-patch fit, step 4). |
| film | `"step_wedge"` | Derived from a transmissive step wedge of known densities on the stock. |
| film | `"none"` | A stub with no measurement. Forces the no-profile fallback; every numeric table must be absent or marked `TODO(measure)`. |

For a rig, the top-level `residual_rms` is the residual of the `[density_fit]` table when present, else `0.0` with a comment, since the flat field and crosstalk tables have no fit residual.

Per-channel values are three-element arrays in R, G, B order. Matrices are three rows, each a three-element array, in R, G, B order for both rows and columns.

## Rig profile: `rig.negg.toml`

A rig profile may carry any subset of the three tables. Each table has its own `method`, `source` and `measured_on` because the three measurements are usually made separately.

### `[flat_field]` (step 2, flat field)

| Field | Type | Meaning |
| --- | --- | --- |
| `method` | string | `"flat_field"`. |
| `source`, `measured_on` | as header | |
| `frame` | string | Path to the blank-light frame `F`, relative to the profile file. Either this or `mean` is required. |
| `mean` | `[r, g, b]` | Stored per-channel `mean(F_c)`, for use when the frame is not shipped. |
| `blur_sigma` | string | The blur rule, written as `"long_side / 64"`. The divisor is fixed by MATHS.md step 2; the field exists so the rule is recorded with the data. |

The gain clamp [0.25, 4] is a constant of the model, not a profile field.

### `[crosstalk]` (step 2, bare-light crosstalk unmix)

| Field | Type | Meaning |
| --- | --- | --- |
| `method` | string | `"crosstalk"`. |
| `source`, `measured_on` | as header | |
| `m` | 3x3 | The matrix `M`, three rows, columns normalised to unit diagonal. The engine applies `M^-1`. |
| `frame_r`, `frame_g`, `frame_b` | string | Paths to the three single-band frames the matrix was built from. |

### `[density_fit]` (step 2, camera-to-density fit)

| Field | Type | Meaning |
| --- | --- | --- |
| `method` | string | `"density_fit"`. |
| `source`, `measured_on` | as header | Say which wedge or grey-card roll, and how its densities were known. |
| `residual_rms` | float | Residual RMS of the least-squares fit, log-D. |
| `n_patches` | integer | Number of patches `N` in the fit. |
| `a` | `[r, g, b]` | Offsets `a_c`. |
| `b` | `[r, g, b]` | Own-channel slopes `b_c`. |
| `c` | 3x3 | Cross terms `c_ck`, row `c`, column `k`. |

### Example rig profile

Every measured value below is `0.0` with a `TODO(measure)` comment. This is a shape, not data; do not copy it into a working profile.

```toml
schema_version = 1
name = "example rig"
source = "TODO(measure): describe the camera, lens, light source and targets"
measured_on = "2026-01-01"
method = ["flat_field", "crosstalk", "density_fit"]
residual_rms = 0.0            # TODO(measure): copied from [density_fit] by the tool

[flat_field]
method = "flat_field"
source = "TODO(measure): blank-light frame, same light and exposure as the roll"
measured_on = "2026-01-01"
frame = "flat.dng"
mean = [0.0, 0.0, 0.0]        # TODO(measure)
blur_sigma = "long_side / 64"

[crosstalk]
method = "crosstalk"
source = "TODO(measure): three frames, one LED band lit at a time"
measured_on = "2026-01-01"
frame_r = "band_r.dng"
frame_g = "band_g.dng"
frame_b = "band_b.dng"
m = [[0.0, 0.0, 0.0],         # TODO(measure): identity would be [[1,0,0],[0,1,0],[0,0,1]]
     [0.0, 0.0, 0.0],
     [0.0, 0.0, 0.0]]

[density_fit]
method = "density_fit"
source = "TODO(measure): transmissive step wedge, densities from the wedge certificate"
measured_on = "2026-01-01"
residual_rms = 0.0            # TODO(measure): reported by negg-cli profile rig
n_patches = 0                 # TODO(measure)
a = [0.0, 0.0, 0.0]           # TODO(measure)
b = [0.0, 0.0, 0.0]           # TODO(measure): identity would be [-1, -1, -1]
c = [[0.0, 0.0, 0.0],         # TODO(measure): identity would be all zeros
     [0.0, 0.0, 0.0],
     [0.0, 0.0, 0.0]]
```

## Film profile: `film.negg.toml`

### `[stock]`

| Field | Type | Meaning |
| --- | --- | --- |
| `name` | string | Stock name as printed on the box. |
| `process` | string | `"C-41"`. Other processes are out of scope until phase 4 (slide film mode). |
| `iso` | integer | Box speed. |

### `[base]` (step 3, Dmin)

| Field | Type | Meaning |
| --- | --- | --- |
| `dmin` | `[r, g, b]` | Film base density `Dmin_c`, log-D. |
| `source` | string | How it was measured (for example, roll-pooled border median over N frames on rig X). |

### `[dmax]` (step 3, Dmax)

| Field | Type | Meaning |
| --- | --- | --- |
| `dmax` | `[r, g, b]` | `Dmax_c`, log-D. |
| `source` | string | How it was measured. |

### `[gamma]` (step 4, layer gamma)

| Field | Type | Meaning |
| --- | --- | --- |
| `gamma` | `[r, g, b]` | `gamma_c`. |
| `gamma_source` | string | `"neutral_fit"` when from the neutral-patch fit of step 4 (the ratios are measured; the absolute value is the user choice recorded here), or `"range"` when it is `Dmax_c - Dmin_c` from the `[dmax]` and `[base]` tables. |
| `source` | string | How it was measured. |

### Example film profile

Every measured value below is `0.0` with a `TODO(measure)` comment. This is a shape, not data; do not copy it into a working profile.

```toml
schema_version = 1
name = "example stock"
source = "TODO(measure): roll with grey card, rig profile used, who measured it"
measured_on = "2026-01-01"
method = "grey_card_roll"
residual_rms = 0.0            # TODO(measure): neutral-fit residual from negg-cli profile film

[stock]
name = "example stock"
process = "C-41"
iso = 0                       # TODO(measure): box speed

[base]
dmin = [0.0, 0.0, 0.0]        # TODO(measure)
source = "TODO(measure): e.g. roll-pooled border median, N frames, rig <name>"

[dmax]
dmax = [0.0, 0.0, 0.0]        # TODO(measure)
source = "TODO(measure): e.g. roll-pooled 99.5th percentile, N frames, rig <name>"

[gamma]
gamma = [0.0, 0.0, 0.0]       # TODO(measure)
gamma_source = "neutral_fit"
source = "TODO(measure): e.g. grey card frame, N patches, residual in header"
```

## Open points

- TODO(spec): the plan does not say whether a film profile's `[base]` and `[dmax]` override the per-frame estimates of step 3 or only seed the roll pool. Until decided, readers should treat them as informational and keep estimating per frame.
- TODO(spec): the plan does not say where profiles are looked up (beside the roll, a user directory, or bundled). The `*.negg.toml` suffix is fixed; the search path is not.
