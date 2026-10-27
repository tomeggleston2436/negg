# negg-session

The negg flows, moved out of `negg-cli` so that the CLI, the desktop app
and the C ABI share them (ADR 0020): settings-file parsing, decode and
prepare, rig and film loading, roll mode (pass 1 and pooling),
`convert_frame_with`, `convert_one` (`negg convert`), `batch_dir`
(`negg batch`) and lock; and the `Session` the desktop app and the C ABI
drive.

## Owns

- Orchestration and I/O of a conversion, a batch and a lock (`convert.rs`,
  `flow.rs`, `roll.rs`, `settings_file.rs`). Progress and warnings are
  reported as `FlowEvent`s (nothing is printed here); `OutputRequest` carries
  the options of `negg convert` and `negg batch`, including `--crop` and
  `--analysis-area`
- The `Session` (`session.rs`): one open roll. `Session::open_roll` reads
  the roll's batch inputs, its `roll.negg.json` and each frame's sidecar and
  decodes nothing; the host then calls `analyse_next` until it returns
  `Done`. Each call analyses one frame (pass 1: decode, rig, analysis grid,
  analysis under the frame's geometry), then pools the roll, and afterwards
  makes any re-pool that an edit left pending
- Views (`view.rs`): `RollView`, `FrameView`, `HistoryView`, `FrameInfo` and
  `NegativeView`, the export request and report. They are serde JSON in snake
  case, like the sidecars. `RollView`, `FrameView` and `HistoryView` carry
  `pool_pending` (an edit changed what the pool sees and the pool has not been
  made again: call `analyse_next` until `Done`, then fetch the views again);
  `FrameInfo` and `FrameView` carry `needs_analysis_area` (the frame's Dmax is
  pinned at the density cap, ADR 0032)
- Previews: tiles of the frame's pyramid (`render_tile`), thumbnails (JPEG),
  scopes, and before pass 1 the decoded negative (`negative_view`,
  `negative_tile`, `negative_thumbnail_jpeg`) for the analysis area tool.
  Tiles and scopes are in the binary wire formats of `wire.rs` (ADR 0022);
  the tile size is `negg_render::TILE_SIZE`. With the `gpu` feature,
  `set_gpu` renders tiles on a `Gpu` (re-exported from `negg-render`), within
  one 8-bit step of the CPU tiles; a failed GPU render falls back to the CPU
  for good, and nothing else uses the GPU (ADR 0036)
- Edits: `preview_frame` (a drag, nothing recorded), `commit_frame`,
  `commit_roll`, `set_roll_analysis_area` (also before pass 1), `lock_frame`,
  `undo` and `redo` over one linear history (ADR 0023). Edits are refused
  with `SessionError::Busy` until pass 1 has finished
- Sidecar writes: `flush` writes the dirty frame sidecars and `roll.negg.json`
  atomically, and never over a sidecar that failed to read or changed on
  disk behind the session (`SessionError::ReadOnly`, `SessionError::Stale`;
  an edit of a frame whose sidecar changed is refused up front). Once the
  pool is final it also rewrites every unedited frame sidecar a roll edit
  made out of date, as `negg batch` would; a session that only
  browsed writes nothing. See ADR 0023
- Warnings: `Session::warnings` and `RollView::pool_warning` say when an
  analysis area or crop change took the film base pool off the clear film
  edge
- Tone presets (ADR 0034): the preset list (built-in plus the user's folder,
  `$NEGG_PRESETS_DIR` else the platform config `negg/presets`), applying a
  preset to a frame or the roll (with the "All frames / Only frames without
  their own look" choice), saving a user preset, and preset previews of a
  frame. A preset changes only grade, toe, shoulder, black point compensation,
  saturation and the `print.preset` label
- `export`: calls `batch_dir` or `convert_one` as the CLI does, so an export
  is byte-identical to `negg batch` or `negg convert` on the same directory
  (ADR 0024). The web copy (ADR 0035) is requested through the export
  request's web mode: original only, both, or web only
- `SessionError` and `ConvertError`

## Must not

- Contain image maths; that lives in the engine crates. Moving pixels
  (RGB to RGBA, `Geometry::apply`) is not maths
- Print to stdout or stderr
- Define the tile size or any other constant that belongs to `negg-render`

Licence: Apache License 2.0 (Apache-2.0). See `LICENSE` in this directory.
