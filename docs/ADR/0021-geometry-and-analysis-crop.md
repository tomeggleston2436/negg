# 0021: geometry, the analysis crop and schema version 3

## Status

Accepted, 2026-10-24 (Rulings 70, 71, 72, 90, 91 and 92; implemented in Wave 0). Amends ADR 0007 (see its "Phase 3 amendment").

## Context

The first real scans were framed with the film holder in shot. The holder is far darker in the scan than any film, so it pins Dmax at the 6.0 cap, the frame renders dark, and the film area is not what the analysis sees. `docs/MATHS.md` steps 3 and 6 computed every statistic on the whole grid. Phase 3 also needs crop, quarter-turn rotation and flip in the app, and they must reach exports. A second crop for analysis would be a second source of truth, so the analysis and the export share one.

## Decision

**One geometry field (Ruling 70).** `Settings.geometry` (`negg-types`, `Geometry`) holds `crop: Option<CropRect>`, `analysis_buffer`, `rotate_degrees` (0, 90, 180 or 270, clockwise) and `flip_horizontal`. There is no second crop anywhere. `CropRect` is normalised to the source frame in its decoded orientation, before any quarter turn or flip; `width` and `height` are in `[0.01, 1]`, `x` and `y` are not negative, and `x + width` and `y + height` are at most `1 + 1e-6`.

**Crop pixels.** For a `W x H` source, `x0 = round(x W)` clamped to `[0, W - 1]` and `x1 = round((x + width) W)` clamped to `[x0 + 1, W]`, likewise for y, rounding half away from zero in `f64`. `None` is the whole frame.

**Export order.** After step 7 (`render_full`) and before the output transform of step 8: the crop itself (no buffer), then the clockwise rotation, then the mirror left to right of the rotated image. It only moves pixels, so it lives in `negg-types` (`Geometry::apply`), not in `negg-render`. An identity geometry (no crop, rotate 0, no flip) skips the copy, so phase 1 and phase 2 exports are byte-identical.

**Analysis crop (Ruling 90).** With `crop = Some(r)` the crop restricts every estimator, and `analysis_buffer` shrinks it further for analysis only. The default is 0.05 (a fraction of the crop's short side in pixels, trimmed on all four sides; valid range `[0, 0.2]`; Controller choice, `TODO(measure)`). The analysis pixel rect is the crop pixel rect shrunk on each side by `round(analysis_buffer * min(x1 - x0, y1 - y0))` pixels, with at least 1 pixel left in each direction. The analysis grid is still built from the whole frame, so block size and cells match the no-crop case. A cell is in a rect when its centre in source pixels, `(i b + min((i + 1) b, W)) / 2` for column `i` and block size `b` (rows alike), lies inside the rect. The crop grid uses the crop rect; the analysis grid uses the buffered rect.

- The crop grid feeds step 3 Dmin, including the border search (Ruling 91), and the Dmin pool candidates.
- The analysis grid feeds step 3 Dmax, the exposure anchor, the step 6 cast fit and every roll pool input derived from the density grid (the `FrameAnalysis` Dmax and `grid_density` used for the cast pool and the gamma).
- Fewer than 64 cells in the analysis grid is `Error::Invalid("crop too small to analyse")` (`MIN_ANALYSIS_CELLS`, Controller choice).
- `crop = None` uses the whole grid with no copy and ignores `analysis_buffer`, so every no-crop output is byte-identical to before.

**Border search inside the crop (Ruling 91).** The border search of ADR 0007 (outer 15% band, flat cells) runs only inside the crop rect, unbuffered: the band is the outer 15% of the crop grid. Cropping tightly to the image usually loses the border source, and Dmin falls back to the 0.5th percentile of the crop grid (the sidecar shows `dmin_source = percentile`); a crop that keeps some rebate keeps the border source. The existing guard (border median at most percentile Dmin plus 0.05) stays meaningful. In roll mode the pooled Dmin from frames that keep their rebate covers tight crops.

**Schema version 3 (Ruling 71).** `SETTINGS_SCHEMA_VERSION` 2 to 3 (adds `geometry`), `SIDECAR_SCHEMA_VERSION` 2 to 3, `ROLL_SIDECAR_SCHEMA_VERSION` 1 to 2. `FrameOverrides` gains `geometry: Option<Geometry>`, and `resolve` replaces the shared geometry when it is `Some`. The sidecar ladder `upgrade_v2_to_v3` sets the two version fields and nothing else (geometry takes its default). The roll sidecar gains `migrate_roll`: version 1 to 2 sets `schema_version = 2` and `settings.shared.schema_version = 3`; a roll version above 2 is rejected. `parse_settings` keeps raising older versions to current. The bump is needed because a phase 2 binary must refuse a file whose crop it would otherwise silently drop. The pinned default settings hash was recomputed.

**CLI (Ruling 92).** `negg convert` and `negg batch` gain `--crop x,y,w,h` (four comma-separated fractions in source orientation, validated as `CropRect`; malformed or invalid is a usage error, exit 2), applied after `--settings` like `--space`. In roll mode it is stored in `roll.negg.json`, so the roll shares one crop; a frame overrides it through `overrides.geometry` in its sidecar. The override is a whole `Geometry` (Ruling 71), so overriding only the crop also replaces `analysis_buffer`, rotation and flip. `analysis_buffer`, rotation and flip are set through `--settings` or the app.

**Straighten deferred (Ruling 72).** Arbitrary-angle rotation needs a resampling kernel in `negg-render` and a decision on its filter, so it goes to phase 4 as `geometry.angle` with a version 4 migration. Phase 3 "rotate" means quarter turns.

## Consequences

- **The holder no longer pins Dmax.** A synthetic negative placed inside a surround at the density cap has Dmax 6.0 without a crop; cropped to the negative with `analysis_buffer = 0` its Dmin, Dmax and gamma are within 0.01 log-D of the same negative uncropped, and with the default buffer within 0.02 (`crates/negg-cli/tests/crop.rs`).
- **Tight crops lose the border source.** One rect means no separate film-base sample outside the crop. Cost of choosing wrongly: an optional `base_rect` field (schema bump).
- **The buffer is a Controller choice.** 0.05 keeps ragged crop edges (dust, a sliver of holder) out of Dmax and the cast fit. It is marked `TODO(measure)` and must not be tuned without a corpus delta.
- **Preview coordinates.** Preview tiles are in source orientation and the crop overlay is drawn in source coordinates; the UI rotates and flips the canvas with a CSS transform. Thumbnails and scopes have geometry applied in Rust (ADR 0022).
- **Per-frame crops.** A frame can override the roll crop; the session re-analyses that frame from its cached grid when the crop or buffer changes (ADR 0023).
- **A phase 2 binary refuses new files.** That is the point of the bump; a v2 file loads in the new binary with identity geometry.
- **Cost of choosing the crop convention wrongly:** it flips to oriented coordinates, a coordinate transform in two places.
- **Schema literals (Ruling 93).** The schema version literals in `crates/negg-cli/tests/convert.rs` and `crates/negg-cli/tests/roll.rs` were updated with the bump, as pure fallout of it.
- **Golden and eval.** With `crop = None` the phase 1 golden render and the `--no-roll` byte test pass unchanged apart from the schema literals.

## Eval delta

None for the corpus and the eval gates: with no crop every render is byte-identical. The cropped synthetic cases above are the test of the behaviour change.
