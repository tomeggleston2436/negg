# negg

negg is an open-source, standalone film negative converter: a Rust engine with
a command-line tool first and a Tauri desktop app second. The aim is to match
or beat Negative Lab Pro on camera-scanned C-41 negatives without needing
Lightroom.

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
- `negg-cli`: the `negg` binary: convert, batch, eval and profile.
- `negg-ffi`: C ABI over the engine (phase 3).

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
negg batch roll/ --out positives            # every RAW or TIFF in roll/, sorted by name, same settings
negg eval --corpus corpus --baseline eval-baseline.json --tolerances eval.toml
```

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
overwritten, and `negg batch` skips any file whose name contains `.negg.`. A RAW is rendered
through its camera matrix into Rec.2020; a TIFF with a matrix/TRC ICC
profile is linearised through its TRC in its own primaries and brought into
Rec.2020 by its colorants after the print stage (LUT profiles are refused);
an untagged TIFF is assumed gamma 2.2 and left in its
own colour space. Exit status: 0 ok, 1 conversion failure (for `batch`, any
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
gives the mean CIEDE2000 per tool (`de2000_mean_by_tool`) and the plan's
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
