# negg-cli

The negg command-line tool: convert, batch, eval and profile.

## Owns

- The `negg` binary and its subcommands (`eval` now; convert, batch and
  profile arrive with the engine)
- `negg eval`: loads and validates `corpus/manifest.toml`, reads each
  reference render, scores CIEDE2000 statistics and grey-card neutrality,
  prints a table and writes `eval-results.json` (schema in
  `src/eval/report.rs`; no timestamps or absolute paths, so CI can diff it)
- The regression gate: `negg eval --baseline` compares the fresh results
  with a previous `eval-results.json` using the tolerances and tracked
  cases in `eval.toml` (`src/eval/compare.rs`, `src/eval/tolerances.rs`)
  and exits with status 3 on a regression
- Argument parsing and orchestration of the engine crates

## Must not

- Contain image, calibration or colour maths (the metrics call into
  `negg-cms` and `negg-decode`)
- Be depended on by any engine crate

Licence: Apache License 2.0 (Apache-2.0). See `LICENSE` in this directory.
