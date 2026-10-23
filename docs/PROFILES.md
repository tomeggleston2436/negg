# negg: rig and film profiles

This document is the schema for the two profile files `negg` reads: a rig profile (`<name>.rig.negg.toml`) describing the scanning setup, and a film profile (`<name>.film.negg.toml`) describing a film stock. Both use the `*.negg.toml` suffix fixed by decision 4 (`docs/ADR/0004-naming.md`). The maths each table feeds is in `docs/MATHS.md`; this document only says how the numbers are stored and where they came from. The rulings behind it are in ADRs 0014 to 0019 (all Proposed). The Rust types are in `negg-types` (`profile.rs`) and the TOML reader and writer are in `negg-io`.

Licence: Apache-2.0 (see `docs/LICENSE`). Profile data committed to this repository is also Apache-2.0 (decision 1).

## Provenance rules

These rules follow ground rule 6 of the plan ("measured, not guessed") and are not negotiable.

1. Every numeric value in a profile needs a `source`: free text saying how it was measured, in the table that holds the value or in the file header when the whole file comes from one measurement. A value with no traceable source does not get committed.
2. No stock-specific constants may be committed without a measurement. There is no "typical Portra 400 Dmin" in this repository. If you cannot cite a measurement, do not ship the number: leave the table out so the fallback applies. Readers reject any `source` that is empty or contains `TODO(measure)`, so a placeholder profile cannot be loaded by mistake.
3. A missing profile, a missing table, or a `method = "none"` film profile means the no-profile fallback in `docs/MATHS.md#step-4-inversion-to-relative-log-exposure`: `gamma_c = Dmax_c - Dmin_c` with `Dmin_c` and `Dmax_c` estimated from the frame or roll per step 3. A missing rig profile means every step 2 correction is the identity.
4. `residual_rms` is reported by the tool that made the fit, in log-D, and is never typed by hand.
5. Profiles are data, not code. Changing a committed profile is a measurement, not a tuning; the commit must say what was re-measured and the `negg-cli eval` delta goes in the pull request like any engine change (ground rule 7).

## Conventions

- Every struct is read with unknown fields denied, so a misspelt key is an error, not a silent default. Every field below is required unless it is marked optional.
- Per-channel values are three-element arrays in R, G, B order. Matrices are three rows, each a three-element array, in R, G, B order for both rows and columns.
- All floats must be finite.
- File names (`frame`, `frame_r`, `frame_g`, `frame_b`) are relative to the directory of the profile file.
- Profile paths stored in `Settings.profiles` resolve relative to the directory of the file that holds them (the roll or frame sidecar). Paths given on the command line are made absolute before they are stored (Ruling 52). In `negg convert` without a roll sidecar, relative `profiles` paths in a `--settings` file resolve against the frame's directory (where its sidecar will be), not the settings file's directory.
- Profiles are found only by explicit path: `--rig FILE`, `--film FILE`, or `Settings.profiles`. There is no search path. The `profiles/` directory in the repository holds committed profiles, with the layout `profiles/rig/<name>.rig.negg.toml` and `profiles/film/<name>.film.negg.toml` (Ruling 46).

## Common header

Both profile kinds start with the same top-level fields.

| Field | Type | Meaning |
| --- | --- | --- |
| `schema_version` | integer | Always `1` for this schema. Readers reject other values. |
| `name` | string | Human-readable name of the rig or stock. |
| `source` | string | Free text: how the profile was measured (equipment, target, procedure). Non-empty, never containing `TODO(measure)`. |
| `measured_on` | string | Date `YYYY-MM-DD`. `negg profile` defaults it to today's UTC date and `--measured-on` overrides it (Ruling 56). |
| `method` | enum string, or array of them for a rig | Which procedure produced the profile; enum below. |
| `residual_rms` | float | Residual RMS of the governing fit, in log-D. |
| `camera` | string, optional (rig) | Free text naming the camera and lens. |
| `rig` | string, optional (film) | Name of the rig profile the film profile was measured with. |

`method` enum:

| Profile | Values | Meaning |
| --- | --- | --- |
| rig | `"flat_field"`, `"crosstalk"`, `"density_fit"` | One per sub-table present. Each sub-table carries its own `method` with the matching value; the top-level `method` lists exactly the sub-tables present, for example `["flat_field", "density_fit"]`. |
| film | `"grey_card_roll"` | Derived from a roll containing a grey card frame by `negg profile film` (neutral-patch fit, step 4). This is the only method phase 2 tools write. |
| film | `"step_wedge"` | Reserved for a profile derived from a transmissive step wedge of known densities on the stock. Readers accept it; no phase 2 tool writes it. |
| film | `"none"` | A stub with no measurement. Forces the no-profile fallback; every numeric table (`[base]`, `[dmax]`, `[gamma]`) must be absent. |

For a rig, the top-level `residual_rms` is the residual of the `[density_fit]` table when present, else `0.0`, because the flat field and crosstalk tables have no fit residual. `camera` is the decoder's camera string when any input frame reports one. The condition number of the density fit is never written: the schema has no field for it, and `negg profile rig` shows it only in the warning it prints when it falls back to the diagonal model.

## Validation

Both readers and writers call `validate`, which rejects:

- `schema_version` other than 1.
- Any `source` that is empty or contains `TODO(measure)`.
- A `measured_on` that is not `YYYY-MM-DD` digits.
- A top-level `method` list that does not match the tables present, or a table whose own `method` does not match.
- Non-finite floats.
- A rig `m` whose diagonal is not 1 within 1e-4, a `c` whose diagonal is not 0, or any `b_c >= 0`.
- A film `ratio[1] != 1` (G), `offset[1] != 0`, or any ratio not above 0.
- `method = "none"` together with any numeric table.
- A `process` other than `"C-41"`.

## Rig profile: `<name>.rig.negg.toml`

A rig profile may carry any subset of the three tables. Each table has its own `method`, `source` and `measured_on` because the three measurements are usually made separately. A rig with no table is valid and is the identity.

### `[flat_field]` (step 2, flat field)

| Field | Type | Meaning |
| --- | --- | --- |
| `method` | string | `"flat_field"`. |
| `source`, `measured_on` | as header | |
| `frame` | string | The blank-light frame `F`, relative to the profile file. It is decoded at run time and must be linear. |
| `mean` | `[r, g, b]` | `mean(F_c)` recorded by the tool at measurement (informational; the runtime recomputes from the frame). |
| `blur_sigma` | string | The blur rule, written as `"long_side / 64"`. The divisor is fixed by MATHS.md step 2; the field exists so the rule is recorded with the data. |
| `width`, `height` | integer | Decoded size of the flat. A frame of any other size is an error (Ruling 37). |

The gain clamp [0.25, 4] is a constant of the model, not a profile field.

### `[crosstalk]` (step 2, bare-light crosstalk unmix)

| Field | Type | Meaning |
| --- | --- | --- |
| `method` | string | `"crosstalk"`. |
| `source`, `measured_on` | as header | |
| `m` | 3x3 | The matrix `M`, three rows, columns normalised to unit diagonal. The engine applies `M^-1` and clips at 0. |
| `frame_r`, `frame_g`, `frame_b` | string | The three single-band frames the matrix was built from, relative to the profile file. |

### `[density_fit]` (step 2, camera-to-density fit)

| Field | Type | Meaning |
| --- | --- | --- |
| `method` | string | `"density_fit"`. |
| `source`, `measured_on` | as header | Say which wedge, and how its densities were known (for example, the certificate). |
| `residual_rms` | float | Residual RMS of the least-squares fit, log-D. |
| `n_patches` | integer | Number of patches `N` in the fit. |
| `model` | string | `"full"` or `"diagonal"`. The diagonal model is used when a neutral wedge or too few patches cannot identify the cross terms (Ruling 39). |
| `a` | `[r, g, b]` | Offsets `a_c`. |
| `b` | `[r, g, b]` | Own-channel slopes `b_c`. Every entry must be negative. |
| `c` | 3x3 | Cross terms `c_ck`, row `c`, column `k`; the diagonal is zero. |

At run time the model is `D_c = a_c + sum_k G_ck log10(max(I_k, 1e-6))` with `G = diag(b) + c`. The identity model is `a = 0`, `b = [-1, -1, -1]`, `c = 0`.

## Film profile: `<name>.film.negg.toml`

### `[stock]`

| Field | Type | Meaning |
| --- | --- | --- |
| `name` | string | Stock name as printed on the box. |
| `process` | string | `"C-41"`. Other processes are out of scope until phase 4 (slide film mode). |
| `iso` | integer | Box speed. |

### `[base]` (step 3, Dmin) optional

| Field | Type | Meaning |
| --- | --- | --- |
| `dmin` | `[r, g, b]` | Roll-pooled film base density `Dmin_c`, log-D. Informational. |
| `source` | string | How it was measured (for example, roll-pooled border median over N frames on rig X). |

### `[dmax]` (step 3, Dmax) optional

| Field | Type | Meaning |
| --- | --- | --- |
| `dmax` | `[r, g, b]` | Roll-pooled `Dmax_c` (median of inliers), log-D. Informational. |
| `source` | string | How it was measured. |

### `[gamma]` (step 4, neutral-patch fit) optional

| Field | Type | Meaning |
| --- | --- | --- |
| `gamma` | `[r, g, b]` | Absolute `gamma_c` at measurement, `ratio_c * gamma_G`. Informational. |
| `ratio` | `[r, g, b]` | `gamma_c / gamma_G`; the G entry is exactly 1. Applied at render. |
| `offset` | `[r, g, b]` | Neutral offset `offset_c` in log-D; the G entry is exactly 0. Applied at render as `offset_c / gamma_G` in `e_c`. |
| `gamma_source` | string | `"neutral_fit"`, the only value phase 2 writes. |
| `n_patches` | integer | Points used in the fit (`NeutralFit.patches`): 2 for a roll with one grey patch (that patch plus the film base), `N` for `N >= 2` grey patches (Ruling 58). |
| `source` | string | How it was measured. |

### What is applied at render

Only `[gamma]` `ratio` and `offset` change a render (Ruling 46). `[base]`, `[dmax]` and the absolute `gamma` are informational, because Dmin and Dmax depend on the rig and the light; the engine keeps estimating them from the frame or roll, with `gamma_G` taken from the frame or the pooled roll (Ruling 45). A profile with no `[gamma]` table, or with `method = "none"`, gives no film correction. `load_film` in `negg-cli` returns an optional correction: a valid profile with `method = "none"` or no `[gamma]` table gives none, and the CLI then warns and renders without a film correction. With a film correction a frame's `gamma_source` is `"profile"` (Ruling 57) and no cast is pooled (the cast is the identity). `"neutral_fit"` appears only in this table, written by `negg profile film`. Using a film profile whose header `rig` differs from the rig profile in use (by name, or one with a rig and the other without) prints a warning.

## Input CSV files

Both files are UTF-8, comma separated, with no quoting. Blank lines and lines starting with `#` are ignored; the first other line is the header. A malformed line is an error that names its line number. Rectangles `x, y, width, height` are fractions of the frame, as in the manifest `grey_card`, and must lie inside [0, 1] (Ruling 42).

`wedge.csv`, for `negg profile rig --wedge W --densities CSV`. The header is exactly `patch,x,y,width,height,density` for a neutral wedge, or `patch,x,y,width,height,density_r,density_g,density_b` for coloured patches. `patch` is a unique label and densities are certified transmission densities, finite and at least 0. Only coloured patches can fit the full density model.

`patches.csv`, for `negg profile film <dir> --patches CSV`. The header is exactly `file,x,y,width,height`, where `file` is a file name in the roll directory and the rectangle marks a neutral patch on that frame.

## Worked examples

The values in these examples are placeholders that show the shape of a file. They are not measurements, they are not typical of any rig or stock, and the `TODO(measure)` text in each `source` makes the readers reject them on purpose (ground rule 6). Do not copy them into a working profile.

### Example rig profile

```toml
schema_version = 1
name = "example rig"
source = "TODO(measure): describe the camera, lens, light source and targets"
measured_on = "2026-01-01"
method = ["flat_field", "crosstalk", "density_fit"]
residual_rms = 0.0            # placeholder: copied from [density_fit] by the tool

[flat_field]
method = "flat_field"
source = "TODO(measure): blank-light frame, same light and exposure as the roll"
measured_on = "2026-01-01"
frame = "flat.dng"
mean = [0.0, 0.0, 0.0]        # placeholder
blur_sigma = "long_side / 64"
width = 0                     # placeholder: decoded width of flat.dng
height = 0                    # placeholder: decoded height of flat.dng

[crosstalk]
method = "crosstalk"
source = "TODO(measure): three frames, one LED band lit at a time"
measured_on = "2026-01-01"
frame_r = "band_r.dng"
frame_g = "band_g.dng"
frame_b = "band_b.dng"
m = [[1.0, 0.0, 0.0],         # placeholder: unit diagonal, no crosstalk
     [0.0, 1.0, 0.0],
     [0.0, 0.0, 1.0]]

[density_fit]
method = "density_fit"
source = "TODO(measure): transmissive step wedge, densities from the wedge certificate"
measured_on = "2026-01-01"
residual_rms = 0.0            # placeholder: reported by negg profile rig
n_patches = 0                 # placeholder
model = "full"
a = [0.0, 0.0, 0.0]           # placeholder
b = [-1.0, -1.0, -1.0]        # placeholder: the identity model
c = [[0.0, 0.0, 0.0],         # placeholder: the identity model
     [0.0, 0.0, 0.0],
     [0.0, 0.0, 0.0]]
```

### Example film profile

```toml
schema_version = 1
name = "example stock"
source = "TODO(measure): roll with grey card, rig profile used, who measured it"
measured_on = "2026-01-01"
method = "grey_card_roll"
residual_rms = 0.0            # placeholder: neutral-fit residual from negg profile film
rig = "example rig"

[stock]
name = "example stock"
process = "C-41"
iso = 0                       # placeholder: box speed

[base]
dmin = [0.0, 0.0, 0.0]        # placeholder
source = "TODO(measure): e.g. roll-pooled border median, N frames, rig <name>"

[dmax]
dmax = [0.0, 0.0, 0.0]        # placeholder
source = "TODO(measure): e.g. roll-pooled Dmax, N frames, rig <name>"

[gamma]
gamma = [0.0, 0.0, 0.0]       # placeholder
ratio = [1.0, 1.0, 1.0]       # placeholder: no film correction
offset = [0.0, 0.0, 0.0]      # placeholder
gamma_source = "neutral_fit"
n_patches = 0                 # placeholder
source = "TODO(measure): e.g. grey card frame, N patches, residual in header"
```

## Decisions recorded here

These resolve the two open points that this document carried before phase 2 (ADR 0016, Proposed).

- A film profile's `[base]` and `[dmax]` are informational. They do not override the per-frame or roll estimates of step 3; only `ratio` and `offset` act at render, and absolute contrast comes from the frame or roll.
- Profiles are found only by explicit path. There is no beside-the-roll, user-directory or bundled search; the `*.negg.toml` suffix is the only convention.
