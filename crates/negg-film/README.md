# negg-film

Film profiles, Dmin/Dmax/gamma estimators and roll pooling.

## Owns

- `analysis_grid` and `AnalysisGrid`: the block-median grid of step 3 of
  `docs/MATHS.md` (about 1024 cells on the long side, per-cell texture
  measure `cv`), `density`, the pure `-log10` transform, and
  `density_value`, the same formula for one sample, which `negg-render`'s
  fused full-resolution render calls per pixel
- `estimate_dmin` (border path per ADR 0007, else the 0.5th percentile,
  reporting which via `DminEstimate`), `estimate_dmax` (99.5th percentile)
  and `no_profile_gamma` (`Dmax - Dmin`, floored at 0.05 log-D)
- `exposure_anchor`: the single-scalar anchor of step 5 per ADR 0008, taken
  on the already inverted grid supplied by the caller
- `percentile` (nearest rank), `median` and `trimmed_mean`, the robust
  statistics everything above is built from
- Film profile data structures and pooling of estimates across a roll
  (phase 2)

Depends on `negg-types` only. The step 4 inversion lives in `negg-render`;
the pipeline that strings grid, estimates, inversion and anchor together
lives there too.

## Must not

- Decode files or do any I/O
- Perform per-pixel rendering
- Duplicate maths that lives in negg-render

Licence: Mozilla Public License 2.0 (MPL-2.0). See `LICENSE` in this directory.
