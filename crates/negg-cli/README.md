# negg-cli

The negg command-line tool: convert, batch, profile, lock and eval.

The conversion flows do not live here. Since phase 3 they are in
`negg-session` (ADR 0020), shared with the desktop app and the C ABI, and this
crate keeps what only a command line needs: argument parsing with clap, the
printing of the flows' `FlowEvent`s with the `negg convert: ` and
`negg batch: ` prefixes, the `--dump-stats` block, exit statuses, `profile`
and `eval`. It re-exports the moved items (`pub use negg_session::...` in
`src/convert.rs` and `src/roll.rs`) so that its tests and callers keep their
paths. The output, stderr text and exit codes are unchanged by the move.

## Owns

- The `negg` binary and its subcommands: `convert`, `batch`, `profile`
  (`rig` and `film`), `lock`, `preset list` and `eval` (`src/cli.rs`,
  `src/main.rs`). Exit status: 0 ok, 1 conversion, measurement or I/O
  failure (for `batch`, any frame), 2 usage, settings, profile or manifest
  error, 3 eval regression
- `negg convert` and `negg batch` (`src/roll.rs`, `src/convert.rs`): each
  maps its arguments to a `negg_session::OutputRequest` and calls
  `negg_session::convert_one` or `batch_dir`, which decode with `negg-decode`,
  linearise and build the step 7 matrix with `negg-cms`, render with
  `negg_render::render_frame`, apply the geometry, export through `negg-cms`
  and write the image and the sidecar with `negg-io`. The CLI prints what the
  flows report; `--dump-stats` (the estimates, `needs analysis area` for a
  frame whose Dmax is pinned at the density cap, and the roll lines) and
  `--timing` are for debugging. Outputs are named `<stem>.negg.tif` or
  `<stem>.negg.jpg` (in `--out`, else beside the source), parallel to the
  `<stem>.negg.json` sidecar, so a TIFF source is never overwritten;
  `negg batch` skips every file whose name contains `.negg.`, refuses an
  output or sidecar path that resolves to its source, and fails a source
  whose output an earlier source in the run already produced
- Crop and analysis area, shared by `convert` and `batch` (ADR 0021):
  - `--crop x,y,w,h`: four fractions of the frame in its source orientation
    (before any rotation). The crop frames the export, and when there is no
    analysis area it is also the area the analysis sees (shrunk by
    `analysis_buffer`, default 0.05). A crop too small to analyse is refused
    with a hint to set a larger analysis area
  - `--analysis-area x,y,w,h`: the same four fractions. Only this area is
    analysed instead of the crop, and the crop, if any, frames the export
    alone; with an area and no crop the export is the whole frame. Cover the
    whole film including its clear edge (rebate), where the film base is
    measured, and leave out only the film holder: when an area (or crop)
    leaves the clear edge out of a roll pooled on it, `negg batch` warns
    that the film base is now estimated from the image (Ruling 138)
  - Both are validated like `Settings.geometry`; a malformed or invalid
    value is a usage error (exit 2). They are applied after `--settings`. In
    roll mode they are the roll's for the run and shared by every frame;
    `negg batch` stores them in `roll.negg.json` (`negg convert` never
    writes the roll sidecar). A frame overrides them through
    `overrides.geometry` in its sidecar, which replaces the whole geometry.
    Rotation, flip and `analysis_buffer` are set through `--settings`
- `--settings FILE` (Rulings 33, 142 to 144): a settings file (the
  `Settings` schema, omitted fields defaulted), a frame sidecar (its
  `settings`) or a roll sidecar `roll.negg.json` (its shared settings). Any
  other shape, and a file whose `schema_version` is newer than this negg, is
  a usage error (exit 2). Without it the settings are the defaults, or in
  roll mode the roll settings stored in `roll.negg.json`. In roll mode the
  file replaces the stored look (film, print, colour, profiles) and keeps
  the stored geometry (crop, analysis area, buffer, rotation, flip) unless
  it has a `geometry` key of its own; `negg batch` warns, naming the parts
  it replaced, and stores the result. `--preset` applies after it
- Tone presets (ADR 0034, Ruling 7), shared by `convert` and `batch`:
  - `--preset NAME|FILE`: a built-in or user preset name (case-insensitive)
    or a `*.preset.negg.toml` file. It sets grade, toe, shoulder, black point
    compensation, saturation and the preset label, and nothing else. An
    unknown name (the message lists the available ones) or an unreadable
    file is a usage error (exit 2). Without roll mode it sets the frame's
    settings, and a frame whose sidecar overrides set its own look keeps it
    with a warning naming the frame. `negg convert` of a roll frame writes
    it into the frame's sidecar overrides, as the app's frame preset does.
    `negg batch` in roll mode makes it the roll's look, stored in
    `roll.negg.json`; a frame with its own look keeps rendering and being
    labelled exactly as before, its whole current look being written into
    its sidecar overrides (a frame with no label gets the label override
    `""`, which means none), as the app's roll preset does for "Frames
    without their own look"
  - `--preset-all-frames`: with `--preset`, `negg batch` in roll mode only
    (a usage error elsewhere): frames with their own look have it cleared,
    so every frame takes the preset, as the app's "All frames". Their other
    overrides stay
  - `negg preset list`: one line per preset, the name, a tab, then
    `(built-in)` or the file it was read from. User presets live in
    `$NEGG_PRESETS_DIR` when set, else `negg/presets` in the platform config
    folder; a file that cannot be used is skipped with a
    `negg preset: warning:` line on stderr
- Web output (ADR 0035), shared by `convert` and `batch`: `--web` also
  writes a web copy of each frame to `<out>/web/<stem>.negg.jpg` (beside the
  sources when there is no `--out`), and `--web-only` writes only that (the
  two conflict, exit 2). The web copy is made from the same render, after
  geometry: an sRGB JPEG with the profile embedded whatever `--space` says,
  quality 85, 2048 px on the long edge and never upscaled. Sidecars are
  written as for any conversion, and `negg batch` never takes the `web/`
  folder's files as inputs
- Roll mode (`negg-session`'s `roll.rs`, printed from `src/roll.rs`):
  `negg batch <dir>` treats the batch inputs of one directory as a roll.
  Pass 1 decodes one frame at a time, applies
  the rig's flat field and unmix and keeps only the frame's analysis; the
  frames are pooled (Dmin, gamma and cast, with outlier rejection); pass 2
  decodes each frame again and renders it with the pooled values. The roll
  settings, the pool and the frame list are written to `roll.negg.json` in
  the directory, and each frame keeps its own sidecar with its overrides.
  A frame that fails in pass 1 is reported and left out of the pool and of
  pass 2. A frame whose Dmax is pinned at the density cap (the film holder is
  in the analysed area) is left out of the gamma and cast pools and a warning
  names it (ADR 0032); an analysis area is the remedy (a frame with its own
  geometry needs it there). `negg convert` uses an existing
  `roll.negg.json` that lists the frame (its settings and pool) and never
  rewrites it
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
  keep its values. When the frame's Dmax is pinned at the density cap (the
  holder was in its analysed area) the lock goes ahead with a warning: the
  locked values come from that analysis, and a locked gamma keeps the frame
  out of the gamma pool for good; set an analysis area and convert again
  before locking
- `negg eval`: loads and validates `corpus/manifest.toml`, renders each
  frame through the engine with default settings (and its manifest
  `geometry`, which has no `analysis_area` yet), registers hand-cropped,
  mirrored or turned references onto the render (ADR 0028), scores CIEDE2000
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
- `tests/`: end-to-end tests on synthetic data. Among them `crop.rs` and
  `analysis_area.rs` (the crop, the analysis area and the pinned-frame pool
  guard), `presets.rs` (`--preset`, `--preset-all-frames` and
  `preset list`), and `app_parity.rs` (Gate 3's automatic half: `Session`
  exports are byte-equal to this binary's `batch` and `convert`, ADR 0024,
  with the roll and frame presets too)

## Must not

- Contain image, calibration or colour maths (the pipeline is
  `negg-render`'s; the metrics call into `negg-cms` and `negg-decode`)
- Hold a conversion flow that the app or the C ABI also needs: that is
  `negg-session`'s
- Be depended on by any engine crate

Licence: Apache License 2.0 (Apache-2.0). See `LICENSE` in this directory.
