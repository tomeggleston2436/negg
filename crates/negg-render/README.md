# negg-render

Inversion, print curve, adjustments and scopes as pure per-pixel functions.

## Owns

- Every per-pixel operation, as pure functions on a buffer plus settings
- Inversion, the print curve and user adjustments
- Scopes (histogram, RGB parade and vectorscope counts) computed from an
  8-bit buffer, the preview pyramid and its tile size (`TILE_SIZE`, 512,
  defined here only), and the display of a decoded negative before analysis

## Must not

- Do any file or device I/O, except the GPU preview kernel behind the
  `gpu` feature, which runs this crate's maths on a GPU device (ADR 0036)
- Decode RAW or TIFF input
- Hold a second copy of maths that lives elsewhere

## Modules

Each module is one step of `docs/MATHS.md`. Every public function's doc
comment names its step.

| Module | Step | Public API |
| --- | --- | --- |
| `invert` | 4 | `invert`, `invert_in_place`, `invert_pixel`: `e_c = -max(D_c - dmin_c, 0) / gamma_c`. Gamma must be finite and positive, else `Error::Invalid`. |
| `print` | 5 | `softplus`, `softplus_inv` (f64 inside, stable at both ends), `grade_k` (`290 / R` clamped to `[2, 10]`, ADR 0006), `solve_x0` (analytic anchor at `P = 0.75`, ADR 0008), `PrintCoefficients::new` (validates like `Settings::validate`, folds `x0 + exposure + cmy_c`), `print_pixel`, `print`. |
| `cast` | 6 | `fit_cast` on the analysis grid per ADR 0009 and ADR 0027 (provisional fit on the lowest-chroma 30% of all cells, then up to 4 refinements that read luma, chroma and hue after the current correction: luma bands at p20/p80, foliage and sky hues and chroma above 0.1 dropped, lowest-chroma 30% per band; OLS of R and B against G, slopes bounded to `[0.5, 2]`, the returned slopes set to 1 when its cells' `e_G` spreads by less than 0.03; identity below 8 provisional cells), `apply_cast`, `apply_cast_pixel`. |
| `matrix` | 7 | `apply_matrix`, `apply_matrix_pixel`: the step 7 entry matrix on the positive, clipped at 0 (BT.2087 Rec.709 to Rec.2020 for RAW, ADR 0026; the profile's colorants for a tagged TIFF). The matrix itself comes from `negg-cms`. |
| `rig` | 2, 3 | `apply_rig_linear`: the flat field gain (bilinear between grid cell centres, flat field gain) then the unmix `I <- max(M^-1 I, 0)`, in place on the caller's step 1 image; `Error::Invalid` on a flat for another frame size. `density_model_pixel`, `density_grid`: the step 3 density from a rig `DensityModel` (`None` is `negg_film::density`). |
| `full` | 3 to 7 | `render_full`: takes linear RGB and runs the step 3 density (`negg_film::density_value`, or the rig density model when one is passed), the inversion minus `Estimates::neutral_offset`, and all of the above fused per pixel, with no intermediate images (not even a full-size density image). Equals the composition of the separate functions (tested within 1e-5). `render_region` renders a `Region` of an image the same way, for preview tiles. |
| `pipeline` | 3 to 7 | `render_frame_with` (returns `FrameRender`: the image, the `Estimates` and the grid size): the one pipeline of ground rule 3. Builds the analysis grid (`ANALYSIS_GRID_LONG_SIDE` = 1024) and its density, restricts the estimators to the analysis rectangle of the settings' geometry (the analysis area, else the crop, else the whole grid; ADR 0021), runs `estimate_frame`, then renders with `render_full`. `estimate_frame` applies the lock precedence with a `RenderContext` (rig density model, film correction, pooled roll estimates). `render_frame` is `render_frame_with` with the default context and renders exactly as phase 1. The estimators are `negg-film`'s; the step 7 matrix is passed in. |
| `saturation` | 7 | `saturate_pixel`: scales the chroma about the pixel's own luma with `print.saturation` (0 to 2, default 1, which returns the pixel unchanged; ADR 0033). The only place saturation is applied; `render_full` calls it per pixel. |
| `resample` | 8 (web) | `downscale_to_long_edge`: the box-filter downscale of a linear image to a given long edge, never upscaling, used for the web copy before the sRGB transform (ADR 0035). |
| `pyramid` | 8 (preview) | `TILE_SIZE`, `downscale_half` (a 2 x 2 linear box mean) and `pyramid`: the levels above level 0 of the preview pyramid, until the long side is at most the tile size (ADR 0022). |
| `scopes` | 8 (preview) | `scopes`, `Scopes`, `SCOPE_BINS`: histogram, parade and vectorscope of an 8-bit sRGB render. Counts only; the wire layout is `negg-session`'s. |
| `negative` | 8 (preview) | `negative_display`, `negative_display_scale`, `negative_display_pixel`: the decoded negative made viewable, not inverted, with one exposure scale per frame (the 99.5th percentile, ADR 0022). Used by the app's pre-analysis step only. |
| `gpu` | 3 to 7, 8 (preview) | Feature `gpu` only (wgpu 30). `Gpu::new`, `Gpu::render_region` (the fused kernel of `render_region` in `render.wgsl`, within `TOLERANCE` = 1/4096 per channel) and `Gpu::render_region_display` (also the sRGB display encoding, RGBA8, within one 8-bit step of the CPU tile). Tested against the CPU kernel in `tests/gpu.rs`; `NEGG_REQUIRE_GPU=1` fails the tests when there is no adapter instead of skipping them (ADR 0036). |
| `roll` | 3, 6 | `analyse_frame`, `analyse_grid`, `pool_roll`, `FrameAnalysis`, `RollFrame`: pass 1 of a frame under its geometry (the analysis area, else the crop, else the whole frame) and the roll pool, which leaves out frames whose Dmax is pinned at the density cap (ADR 0032). |

Per-image functions run over row bands with rayon. The per-pixel kernels
(`*_pixel`) are shared between the separate functions and `render_full`, so
the two paths are bitwise identical.

## Decisions taken here

- `invert`, `invert_in_place` and `render_full` return `Result` because the
  contract asks for gamma to be validated.
- Step 5 takes `x_c = e_c * k + x_0 + exposure + cmy_c` (ADR 0011): paper
  exposure is the negative's transmittance, so a denser negative (more
  negative `e_c`, a brighter scene) prints brighter, and `print_pixel` is
  monotone non-increasing in `e`. `solve_x0` is `x_target - anchor * k`.
- `apply_cast_pixel` treats a zero or non-finite slope as the identity for
  that channel, since a `CastCorrection` can arrive from a sidecar.
- In `fit_cast`, a cell whose chroma is below `1e-4` has no hue, so it can
  fall in neither the foliage nor the sky sector and is kept: it is the
  most neutral cell available. A perfectly neutral grid fits the identity
  with every selected cell counted in `cells_used`.
- The lowest-chroma 30% of a band is rounded up, so a non-empty band keeps
  at least one cell.
- When a fitted slope is bounded or not identified, the offset is
  recomputed through the centroid of the selected cells so that they still
  land on `e_G` on average.
- The nearest-rank percentile used for the luma bands is a private helper
  here because `negg-render` cannot depend on `negg-film`.
- With a rig density model, Dmin and Dmax are found by running the
  `negg-film` estimators on a grid whose block medians are `10^-D` of the
  model density (and whose `cv` is the original grid's), so the estimators
  keep their single copy in `negg-film`. The round trip costs about one
  ulp, and a model density above 6 reads back as 6.
- In `estimate_frame`, a film correction's offsets apply whenever one is
  in use, divided by the green gamma in effect: the film's `gamma_G`, or a
  fixed green gamma when gamma is locked. A locked render thus reproduces
  the unlocked one.
- A fixed `colour.cast` wins over `auto_colour` off and over a film
  correction (contract precedence). The fixed cast records `cells_used = 0`.

Licence: Mozilla Public License 2.0 (MPL-2.0). See `LICENSE` in this directory.
