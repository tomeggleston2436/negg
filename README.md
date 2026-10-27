# negg

negg is an open-source, standalone film negative converter: a Rust engine with
a command-line tool first and a Tauri desktop app second. The aim is to match
or beat Negative Lab Pro on camera-scanned C-41 negatives without needing
Lightroom.

## Status

Phases 0 to 3 are built: the engine and the `negg` CLI (phases 1 and 2: inversion,
roll pooling, rig and film profiles, `negg eval`) and the desktop app with its
C ABI (phase 3). The gates are not claimed met. The committed corpus has no
frames and there is no rig, so Gates 1 and 2 are not met by data, and Gate 3
(a tester converts a 36-frame roll in the app) is open: its automatic half, that
the app's exports are byte-identical to the CLI's, is a test
(`crates/negg-cli/tests/app_parity.rs`, ADR 0024). Phase 4 has begun with
tone presets, saturation and web export (ADR 0033 to 0035); the built-in preset
values are provisional. Linear DNG output was dropped and looks as
3D LUTs are parked; arbitrary-angle straightening remains. The app's preview renders on the GPU when there is one (ADR 0036).

## Crates

All crates live under `crates/` and each has its own `README.md` stating what
it owns and what it must not do.

- `negg-types`: `Image<f32>`, `Settings`, `RollSettings` and errors; no
  dependencies beyond serde.
- `negg-decode`: RAW and TIFF input to linear camera RGB `f32`.
- `negg-calib`: rig profiles: flat field, crosstalk, camera-to-density fit.
- `negg-film`: film profiles, Dmin/Dmax/gamma estimators, roll pooling.
- `negg-render`: inversion, print curve, adjustments and scopes.
- `negg-cms`: working space, ICC profiles and soft proof.
- `negg-io`: TIFF, DNG and JPEG writers plus sidecars.
- `negg-session`: the flows (convert, batch, lock, roll mode) and the
  `Session` that the app and the C ABI drive; orchestration and I/O, no image
  maths.
- `negg-cli`: the `negg` binary: convert, batch, lock, eval and profile, on top
  of `negg-session`.
- `negg-ffi`: C ABI over `negg-session` for hosts that are not Rust (phase 3).

The desktop app is in `app/` (see `app/README.md`). It is its own cargo
workspace (`app/src-tauri`), so `cargo build --workspace` at the root never
builds Tauri or needs Node.

## Licensing

The engine crates (`negg-types`, `negg-decode`, `negg-calib`, `negg-film`,
`negg-render`, `negg-cms`, `negg-io`) are licensed under the Mozilla Public
License 2.0; see `LICENSE-MPL-2.0`. `negg-cli`, `negg-ffi`, the desktop app,
the documentation and the profile and corpus data are licensed under the
Apache License 2.0; see `LICENSE-APACHE-2.0`. Each crate directory carries its
own `LICENSE` file and every source file starts with an SPDX header.

Dependencies must be MIT, Apache-2.0, BSD, MPL-2.0 or LGPL. No GPL or AGPL
code is permitted. `rawler` (LGPL-2.1) is used as an ordinary crate
dependency and never vendored; see `NOTICE`. The policy is enforced by
cargo-deny using `deny.toml`.

## Building

The toolchain is pinned to stable in `rust-toolchain.toml`. Install
[cargo-deny](https://github.com/EmbarkStudios/cargo-deny) for the last step.

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all --check
cargo deny check
```

All five must pass before a change is considered done.

## Usage

```sh
negg convert frame.dng                      # frame.negg.tif (16-bit, sRGB, ICC embedded) and frame.negg.json beside it
negg convert frame.dng --out positives --format jpeg --space adobe_rgb
negg convert frame.dng --settings my.json --dump-stats --timing
negg convert frame.dng --crop 0.04,0.06,0.92,0.88            # crop the export; the analysis sees the crop too
negg convert frame.dng --analysis-area 0.1,0.1,0.8,0.8       # analyse the film only; the export is not cropped
negg batch roll/ --out positives            # every RAW or TIFF in roll/ as one roll: pooled film base, gamma and cast
negg batch roll/ --no-roll                  # each frame on its own, no roll.negg.json
negg lock frame.dng --dmin --gamma          # keep this frame's estimates in later batches
negg batch roll/ --preset Rich              # the Rich tone preset for the roll; frames with their own look keep it
negg preset list                            # the built-in and user tone presets
negg convert frame.dng --preset Soft --web  # a tone preset, plus web/frame.negg.jpg (2048 px, JPEG 85, sRGB)
negg profile rig --flat flat.dng --name rig1 --source "..." --out rig1.rig.negg.toml
negg profile film roll/ --patches patches.csv --stock Portra --iso 160 --name p160 --source "..." --out p160.film.negg.toml
negg eval --corpus corpus --baseline eval-baseline.json --tolerances eval.toml
```

`--crop x,y,w,h` and `--analysis-area x,y,w,h` take four fractions of the frame
in its source orientation. A scan with the film holder in shot pins Dmax at the
density cap and renders dark; an analysis area drawn on the film keeps the
holder out of the estimates without cropping the photo (ADR 0021). A batch
warns for each frame it finds pinned and leaves it out of the gamma and cast
pools (ADR 0032). In roll mode both are stored in `roll.negg.json` and shared by
every frame. `negg batch` also takes `--rig` and `--film` profile files, as
does `convert`.

`--dump-stats` prints the estimates used for the frame (Dmin and its source,
Dmax, gamma and its source, the exposure anchor and `x0`, the cast fit) plus
the grid and image sizes; `--timing` prints per-stage milliseconds to stderr.
The settings file is JSON in the `Settings` schema of `negg-types`; omitted
fields take their defaults, for example
`{"print": {"grade": 90}, "colour": {"auto_colour": false}}`. A sidecar
(recognised by its top-level `settings` key) is also accepted, so a frame's
`<stem>.negg.json` re-renders another frame with the same settings; a JSON
object with neither is refused with exit status 2. The sidecar
`<stem>.negg.json` records the settings, their hash and the estimates, so a
render is reproducible from the source plus that file. Outputs are named
`<stem>.negg.tif` or `<stem>.negg.jpg`, so a TIFF source is never
overwritten, and `negg batch` skips any file whose name contains `.negg.`. A RAW's print
channels are read as Rec.709 primaries and enter Rec.2020 through the BT.2087
matrix (ADR 0026), not the camera matrix; a TIFF with a matrix/TRC ICC
profile is linearised through its TRC in its own primaries and brought into
Rec.2020 by its colorants after the print stage (LUT profiles are refused);
an untagged TIFF is assumed gamma 2.2 and left in its
own colour space. Tone presets (ADR 0034) set only the print grade, toe, shoulder, black point
compensation and saturation, plus a label recorded in `print.preset`. The
built-ins are Standard, Soft, Hard, Flat, Rich and Linear (provisional values).
`--preset <NAME|FILE>` takes a built-in or user preset name or a
`*.preset.negg.toml` file and applies after `--settings`. User presets are
those files in `$NEGG_PRESETS_DIR` when set, else the `negg/presets` folder of
the platform config directory; each is flat TOML with `schema_version = 1`, `name`, `grade`, `toe`,
`shoulder`, `black_point_compensation` and `saturation`.
In `negg batch` roll mode, frames that have their own look keep it;
`--preset-all-frames` replaces them too. `--web` also writes a web copy and
`--web-only` writes only that: 2048 px on the long edge (never upscaled), 8-bit
JPEG at quality 85, sRGB with the ICC profile embedded whatever the output
space, at `web/<stem>.negg.jpg` beside the originals or under `--out`
(ADR 0035). Saturation (`print.saturation`, 0 to 2, default 1; ADR 0033) is
neutral at its default, so renders without a preset or a saturation change are
byte-identical to earlier phases. Settings files are schema 5, frame sidecars
5 and roll sidecars 4; older files migrate on read and newer ones are refused.
Exit status: 0 ok, 1 conversion failure (for `batch`, any
frame), 2 usage or settings error, 3 eval regression.

## Evaluation

`negg eval` decodes every frame in `corpus/`, renders it through the engine
with default settings and scores the render: CIEDE2000 against each reference
render (mean, p95, max; on a shared block-median grid when the sizes differ),
grey-card neutrality (`(|a*| + |b*|) / 2` on the card patch of the render)
and its signed counterpart `neutrality.de = sqrt(mean(a*)^2 + mean(b*)^2)`,
the chroma of the patch's mean colour), and per-roll variance of the
estimated Dmin and gamma per channel (present when a roll has two or more
evaluated frames). When a frame has both a negadoctor and a NegPy reference,
their distance to each other is recorded as `reference_distance`. The summary
gives the mean CIEDE2000 per tool (`de2000_mean_by_tool`) and the
Gate 1 criteria under `gate1`: `engine_vs_negpy` and `negadoctor_vs_negpy`
(both over the frames that have both references), `neutrality_de_mean`, and
`pass`, which is present when both distances are and is true when the engine
is closer to NegPy than negadoctor is and the mean neutrality error is under
dE 4; fields with no data are null. Each frame's estimates are written into
the results. It prints a table and writes `eval-results.json`. Corpus frames are local-only, so entries whose files are
absent are skipped and counted, and CI evaluates zero frames until the corpus
is shared (docs/ADR/0005). CI runs it on every pull request with `--baseline
eval-baseline.json --tolerances eval.toml` and fails (exit status 3) when any
metric exceeds its baseline by more than its tolerance. To accept a new
baseline, copy a fresh `eval-results.json` over `eval-baseline.json` in a
reviewed commit that explains the delta. When a frame renders badly, add it as
a `[[case]]` in `eval.toml` first, so the fix is measured and cannot regress.

The committed corpus has no frames: frames and reference renders are
local-only (docs/ADR/0005). Personal scans that must not be published go in a
private corpus outside the repository, with the licence `LicenseRef-Private`,
and are evaluated with their own `--corpus`, `--out` and `--baseline` (see
`corpus/README.md`). A frame's manifest `geometry` can carry a crop, a buffer,
a turn and a flip but not yet an `analysis_area`.
