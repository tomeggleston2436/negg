# negg-cli

The negg command-line tool: convert, batch, profile, lock and eval.

## Owns

- The `negg` binary and its subcommands: `convert`, `batch`, `profile`
  (`rig` and `film`), `lock` and `eval`
- `negg convert` and `negg batch` (`src/convert.rs`): decode with
  `negg-decode`, linearise and build the camera matrix with `negg-cms`,
  render with `negg_render::render_frame`, export through `negg-cms` and
  write the image and the sidecar with `negg-io`; `--dump-stats` and
  `--timing` for debugging. Outputs are named `<stem>.negg.tif` or
  `<stem>.negg.jpg` (in `--out`, else beside the source), parallel to the
  `<stem>.negg.json` sidecar, so a TIFF source is never overwritten;
  `negg batch` skips every file whose name contains `.negg.`, refuses an
  output or sidecar path that resolves to its source, and fails a source
  whose output an earlier source in the run already produced
- Roll mode (`src/roll.rs`): `negg batch <dir>` treats the batch inputs
  of one directory as a roll. Pass 1 decodes one frame at a time, applies
  the rig's flat field and unmix and keeps only the frame's analysis; the
  frames are pooled (Dmin, gamma and cast, with outlier rejection); pass 2
  decodes each frame again and renders it with the pooled values. The roll
  settings, the pool and the frame list are written to `roll.negg.json` in
  the directory, and each frame keeps its own sidecar with its overrides.
  A frame that fails in pass 1 is reported and left out of the pool and of
  pass 2. `negg convert` uses an existing `roll.negg.json` that lists the
  frame (its settings and pool) and never rewrites it
- Profile and roll flags, shared by `convert` and `batch`:
  - `--rig FILE`: a rig profile (`*.rig.negg.toml`: flat field, crosstalk
    unmix, density fit)
  - `--film FILE`: a film profile (`*.film.negg.toml`) whose gamma ratios
    and neutral offsets are applied. A profile with method `none` or no
    `[gamma]` table gives no correction: a warning is printed and the frame
    renders without one. A warning is also printed when the profile was
    measured with a different rig than the one in use
  - `--no-roll`: no pooling and no roll sidecar, each frame on its own as
    in phase 1; with no profiles and no frame overrides the output is byte
    for byte the phase 1 output. Frame sidecar overrides and `--rig` and
    `--film` still apply
  - Both profile paths are stored in the settings as absolute paths. A
    relative path inside a `--settings` file resolves against the frame's
    directory (where its sidecar is)
- `negg profile rig`: measures a rig profile from any subset of `--flat`,
  `--band-r --band-g --band-b` (all three or none) and `--wedge` with
  `--densities wedge.csv`, with required `--name`, `--source` and `--out`
  and optional `--measured-on YYYY-MM-DD` (default today, UTC). Frames
  must be linear (RAW or float TIFF) and are written relative to, and
  under, the output directory. The density fit falls back to the diagonal
  model, with a warning that names the reason, when there are too few
  patches or the design matrix is ill conditioned. The fit's condition
  number is not stored in the profile
- `negg profile film <dir> --patches patches.csv --stock NAME --iso N`:
  measures a film profile from a roll containing grey patches, with
  required `--name`, `--source` and `--out`, optional `--measured-on` and
  `--rig FILE`. It pools the roll, fits the neutral patches and writes the
  gamma ratios and offsets. Every written profile is validated first, so a
  `TODO(measure)` source is refused
- `negg lock <frame> [--dmin] [--gamma] [--cast]`: writes the frame's
  current estimates into its sidecar overrides as Fixed (none of the flags
  means all three) and rewrites the sidecar's resolved settings and hash.
  A locked frame neither feeds nor takes the roll pool, and later batches
  keep its values
- `negg eval`: loads and validates `corpus/manifest.toml`, renders each
  frame through the engine with default settings, scores CIEDE2000
  statistics against the reference renders, grey-card neutrality on the
  render and per-roll variance of the estimates, prints a table and
  writes `eval-results.json` (schema in `src/eval/report.rs`; no
  timestamps or absolute paths, so CI can diff it)
- `tests/perf.rs`: the 24 MP under 2 s gate, run in release with
  `cargo test --release -p negg-cli --test perf -- --ignored --nocapture`
- The regression gate: `negg eval --baseline` compares the fresh results
  with a previous `eval-results.json` using the tolerances and tracked
  cases in `eval.toml` (`src/eval/compare.rs`, `src/eval/tolerances.rs`)
  and exits with status 3 on a regression
- Argument parsing and orchestration of the engine crates

## Must not

- Contain image, calibration or colour maths (the pipeline is
  `negg-render`'s; the metrics call into `negg-cms` and `negg-decode`)
- Be depended on by any engine crate

Licence: Apache License 2.0 (Apache-2.0). See `LICENSE` in this directory.
