# 0019: Gate 2 metrics and measured profiles without a corpus

## Status

Proposed, 2026-10-23.

## Context

Gate 2 in `ROADMAP.md` reads "grey-card neutrality under dE 2 on the same roll scanned under three different light sources with a rig profile; roll-level Dmin variance under 0.02 log-D across inlier frames". The unit and statistic of "variance under 0.02 log-D" are not stated (variance has units of log-D squared), nor is what qualifies as "the same roll under three lights". The plan also lists "first measured film profiles from the corpus (at least the four stocks)". The corpus currently holds zero frames, and ground rule 6 forbids committing a number without a measurement.

## Decision

- Gate 2: read Dmin variance "under 0.02 log-D" as the population standard deviation (unit log-D) of the per-frame (unpooled) Dmin over the roll's Dmin inliers, per channel; the gate value is the maximum over channels and over rolls with at least 2 inliers. For three lights: frames carry an optional manifest `film_roll`; a film roll qualifies when its evaluated grey-card frames span at least 3 distinct `light_source` values and every one of those frames has a `rig`. Per light, take the mean `neutrality_de` of its card frames; the gate value is the maximum over lights and qualifying film rolls. `gate2.pass = dmin_std_max < 0.02 && three_lights_max_de < 2.0`, and is null when either is null. Roll-to-roll metric: per stock with at least 2 rolls, the population standard deviation across rolls of the pooled Dmin (per channel) and of the pooled gamma ratios R/G and B/G; it is reported and tolerance-gated, but is not a Gate 2 term. The eval renders each manifest roll in roll mode with its `rig` and `film_profile`.
- Measure: the corpus has zero frames, so "first measured film profiles" ships tooling only: `profiles/` with a README and empty `rig/` and `film/` directories, readers that reject any `TODO(measure)` source, and a test that every committed `*.negg.toml` there loads and validates (zero files passes). No numbers are committed. The roadmap item and Gate 2 stay open until frames and a rig exist.

## Consequences

- With no frames, `gate2.pass` is null, never true. A null is not a pass.
- Using the standard deviation makes the 0.02 threshold a spread in log-D, directly comparable with the 0.3 log-D reject distance.
- The eval tolerances `dmin_std`, `roll_to_roll_dmin_std` and `three_lights_de` ship as placeholders in `eval.toml` and are `TODO(measure)`.
- There is nothing to undo if the corpus later shows the thresholds are wrong; only report fields change.

Cost if wrong: gate 2, report fields; measure, none (nothing to undo).

## Eval delta

`REPORT_SCHEMA_VERSION` goes to 2. The baseline is regenerated from the empty corpus, so every new metric is null or empty.
