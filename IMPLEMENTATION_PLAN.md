# negg implementation plan

An open-source, standalone film negative converter: a Rust engine (`negg`) with a CLI first and a Tauri 2 desktop app second. Target: match or beat Negative Lab Pro on camera-scanned C-41 negatives without Lightroom.

This file is the brief for Claude Code. Read it fully before touching anything. Work phase by phase, in order. Do not start a phase until the previous phase's gate passes in CI.

## Ground rules

1. **Clean-room only.** Do not open, read, or paste from darktable (`negadoctor.c`), RawTherapee (`filmnegativeproc.cc`), NegPy, or FreeCCR. They are GPL/AGPL. The maths you need is written out in this file. If something here is ambiguous, ask rather than looking it up in those repos. DarkSlide is MIT and may be consulted for its binary IPC format only.
2. **Licence.** MPL-2.0 for engine crates, Apache-2.0 for CLI, FFI, app and data (see Decisions at the end). Every dependency must be MIT, Apache-2.0, BSD, MPL-2.0 or LGPL (LGPL linked as a normal crate dependency, never vendored into our source). No GPL, no AGPL. Add `cargo deny` with this policy in phase 0.
3. **One pipeline.** Every pixel operation lives in `negg-render` as a pure function on a buffer plus a settings struct. No second copy in TypeScript, WGSL or anywhere else. When a GPU path arrives (phase 4) it is tested against the CPU path on every CI run.
4. **Float32 end to end.** Pixel data is `f32` linear from decode to the final encode. Quantisation happens exactly once, in the exporter. Never route analysis through an 8-bit buffer.
5. **Statistics are robust.** Film base, Dmax, neutral estimation all use percentiles on a block-median grid, never raw min/max or a plain mean of a rectangle. Dust and scratches must not move the result.
6. **Measured, not guessed.** No per-film-stock magic constants typed from memory. A film or rig profile is a TOML file with a `source` field saying how it was measured. If you cannot cite a measurement, do not ship the number; put a `TODO(measure)` and use the no-profile fallback.
7. **Evaluation before features.** Any change to the engine runs `negg-cli eval` in CI and must not regress the corpus metrics.
8. **Small PRs, conventional commits, tests with every change.** Prefer a 200-line PR with a test over a 2,000-line PR.
9. British English in docs and UI strings; no em dashes anywhere.

## Repository layout

```
negg/
  Cargo.toml                 # workspace
  deny.toml                  # cargo-deny licence policy
  IMPLEMENTATION_PLAN.md     # this file
  docs/
    MATHS.md                 # the model, kept in sync with code
    PROFILES.md              # rig and film TOML schema
    ADR/                     # architecture decision records, one per decision
  corpus/                    # git-lfs; RAW frames + references + manifest.toml
  crates/
    negg-types/           # Image<f32>, Settings, RollSettings, errors; no deps beyond serde
    negg-decode/          # RAW/TIFF -> linear camera RGB f32
    negg-calib/           # rig profiles: flat field, crosstalk, camera-to-density fit
    negg-film/            # film profiles, Dmin/Dmax/gamma estimators, roll pooling
    negg-render/          # invert, print curve, adjustments, scopes
    negg-cms/             # working space, ICC, soft proof
    negg-io/              # TIFF/DNG/JPEG writers, sidecars
    negg-cli/             # `negg` binary: convert, batch, eval, profile
    negg-ffi/             # C ABI (phase 3)
  app/                       # Tauri 2 + React + TypeScript (phase 3)
```

Each crate has its own `README.md` stating what it owns and what it must not do.

## The model (implement this, in this order)

Notation: per channel c in {R, G, B}; `I_c` is linear camera RGB in [0, 1] after decode and rig calibration; all logs base 10.

### Step 1: decode to linear camera RGB

- Use `rawler` for RAW. Run demosaic and active-area crop only. Do **not** apply white balance, the camera colour matrix, or any sRGB conversion. Output is camera-native RGB at unity gain.
- Scale so that the sensor's calibrated white level maps to 1.0 and black level to 0.0, per channel. Use the camera's documented linear limit, not the per-frame maximum, so every frame of a roll decodes on the same scale.
- Record the camera-to-XYZ matrix from rawler for later use (phase 2), but do not apply it here.
- 16-bit TIFF input: honour the embedded ICC if matrix/TRC (linearise with its TRC); otherwise assume linear if the file says so, else gamma 2.2 with a warning. Never downconvert to 8-bit.

### Step 2: rig calibration (phase 2; identity until then)

- **Flat field:** `I_c <- I_c * mean(F_c) / blur(F_c)` where F is a blank-light frame, blurred with sigma = long side / 64, gain clamped to [0.25, 4].
- **Bare-light crosstalk unmix (narrow-band lights):** from three frames each lit by one LED band, build matrix M with columns normalised to unit diagonal; apply `I <- M^-1 I` in linear space, clip at 0.
- **Camera-to-density fit (the new piece):** given N patches of known density (a transmissive step wedge, or a grey card on film whose densities are estimated from the roll), fit `D_c = a_c + b_c * log10(I_c) + sum_k c_ck * log10(I_k)` (3x3 in log space plus offset) by least squares. Store in the rig profile. Report residual RMS.

### Step 3: density and film base

- `D_c = -log10(max(I_c, 1e-6))`.
- Analysis grid: downsample to about 1024 px on the long side using block medians (not means), then compute all statistics on the grid.
- **Dmin (film base) per channel:** if the frame has a detectable unexposed border, use the median of flat border cells (cell std/mean < 0.03) in the outer 15% band; else the 0.5th percentile of D on the grid, per channel. Expose which source was used.
- **Dmax per channel:** 99.5th percentile of D on the grid.
- **Roll pooling (phase 2):** for frames in a roll, Dmin is the per-channel median across frames after rejecting frames whose Dmin lies more than 0.3 log-D from the median. Same for per-channel gamma below.

### Step 4: inversion to relative log exposure

Density above base: `d_c = D_c - Dmin_c`, clamp at 0.

Per-channel contrast normalisation: `e_c = -d_c / gamma_c`, where `gamma_c` is the layer gamma. Default `gamma_c = (Dmax_c - Dmin_c)` so each channel spans [0, -1] (equivalent to independent per-channel stretch). With neutral samples available, fit instead: given N >= 2 patches that should be neutral with densities `d_c,n`, solve for `gamma_R/gamma_G` and `gamma_B/gamma_G` and offsets `o_R, o_B` by least squares on `d_R,n / gamma_R + o_R = d_G,n / gamma_G`, etc. (Two patches give an exact solution; more give a fit. The ratio is identifiable; absolute contrast is a user choice.)

Linear positive: `T_c = 10^(e_c)` in (0, 1].

### Step 5: print stage (the tone curve)

Work in print density `P_c`:

```
x_c   = -log10(T_c) * k + x_0 + cmy_c            # k: grade (contrast), x_0: exposure, cmy: colour timing in log space
v_c   = Dmin_p + softplus(a_hl * (x_c - Dmin_p)) / a_hl           # shoulder into paper white
P_c   = Dmax_p - softplus(a_sh * (Dmax_p - v_c)) / a_sh           # toe into paper black
out_c = 10^(-P_c)                                                 # scene-linear reflectance
```

Defaults: `Dmin_p = 0.06`, `Dmax_p = 2.3`, `a_hl = 3.0`, `a_sh = 6.0`. Grade `k` expressed to the user as an ISO-R style number (default 115, range 50 to 180) with `k = 2.9 * range / (R/100)`, clamped to [2, 10]. Exposure `x_0` chosen so the frame's exposure anchor (trimmed mean of textured grid cells in the midtones) prints at P = 0.75. Optional black point compensation: `out_c <- (out_c - 10^-Dmax_p) / (1 - 10^-Dmax_p)`.

`softplus(z) = ln(1 + e^z)`. Precompute all per-channel coefficients once per render so the per-pixel kernel is one log, two softplus, one pow.

### Step 6: auto colour (cast removal)

On the normalised log image from step 4, take three luma bands (shadows, mids, highlights by 20/50/80th percentile), in each take the lowest-chroma 30% of cells excluding hue sectors for foliage (green, 90 to 160 degrees) and sky (blue, 200 to 260 degrees). Fit R and B against G as a line through those cells (slope and offset, clamped slope deviation to plus or minus 0.2). Apply as per-channel log-affine corrections before step 5. Roll mode (phase 2): pool the fit across inlier frames by confidence-weighted median.

### Step 7: colour management

Working space for all post-inversion adjustments: linear Rec.2020, entered via the camera matrix from step 1 (camera RGB -> XYZ -> Rec.2020) applied **after** inversion to `out_c`, Bradford-adapted to D65. Until phase 2's rig fit exists this is the best available characterisation and it is what NLP effectively gets from Adobe's DCP. Export via lcms2 to sRGB, Adobe RGB, Display P3, ProPhoto or Rec.2020 with the ICC embedded. Display: convert to the monitor profile when known, else sRGB.

### Step 8: export

16-bit TIFF (deflate, ICC tag 34675, orientation 1, resolution tags), 8-bit JPEG with ICC, linear DNG (phase 4). Sidecar `frame.negg.json` beside the source: settings, estimated Dmin/Dmax/gammas, their source, and a `schema_version`.

Write `docs/MATHS.md` from these eight steps before writing code, then keep it in sync. Any deviation needs an ADR.

## Phases, tasks, and gates

### Phase 0: corpus, harness, skeleton (target: 4 weeks)

- [ ] Workspace with all crates as empty libs, `cargo deny`, CI on Linux/macOS/Windows (`cargo test`, `cargo clippy -D warnings`, `cargo fmt --check`, `cargo deny check`).
- [ ] `negg-types`: `Image { width, height, channels: 3, data: Vec<f32> }` with tiled iteration helpers; `Settings` and `RollSettings` as serde structs with `#[serde(default)]` and a content hash.
- [ ] `corpus/manifest.toml` schema: id, file, stock, camera, light source, has_grey_card, reference renders, notes. Add git-lfs. Seed with whatever frames are available; the target is 30+ across 4 stocks, 3 cameras, 2 lights, with grey cards where possible. Document how contributors add frames.
- [ ] `negg-cli eval`: for each corpus frame render with default settings, compute (a) CIEDE2000 against reference renders where present, (b) grey-card neutrality error (mean |a*|, |b*| of the card patch), (c) per-roll variance of estimated Dmin and gammas. Print a table and write `eval-results.json`. Until the engine exists, this runs against the reference renders themselves and reports zero; the point is the plumbing.
- [ ] CI uploads `eval-results.json` and fails if any metric regresses by more than a tolerance set in `eval.toml`.
- [ ] `docs/MATHS.md` and `docs/PROFILES.md` written from this plan.

**Gate 0:** eval runs in CI on every PR and the corpus has at least 10 frames with references.

### Phase 1: core engine and CLI (target: 8 weeks)

- [ ] `negg-decode`: rawler integration per step 1; `tiff` crate for TIFF input; unit tests with a synthetic DNG; integration test on corpus frames checking white level pinning (same frame decoded twice gives identical data; two frames of one roll share a scale).
- [ ] `negg-film`: analysis grid (block median), Dmin and Dmax estimators per step 3, exposure anchor, no-profile per-channel gamma. Tests: synthetic negatives with known Dmin recover it within 0.01 log-D; adding 2% random dust pixels changes the estimate by less than 0.005.
- [ ] `negg-render`: steps 4 to 6 as pure functions; precomputed coefficients; rayon over rows. Property tests: monotone in input per channel; a neutral grey ramp inverts to a neutral ramp.
- [ ] `negg-cms`: Rec.2020 working space entry via the camera matrix; lcms2 transform to output profiles; the four bundled v4 ICC profiles (sRGB, Adobe RGB compatible, Display P3, Rec.2020) from a permissive source or generated from primaries.
- [ ] `negg-io`: 16-bit TIFF with ICC; JPEG; sidecar read/write with schema versioning.
- [ ] `negg-cli convert <raw> [--out dir] [--settings file]` and `batch <dir>`; `--dump-stats` prints Dmin/Dmax/gammas/anchor for debugging.
- [ ] Reference renders for the corpus from darktable (negadoctor auto pickers, default settings) and NegPy (defaults) as `reference/*.tif`, made outside this repo and committed via lfs.

**Gate 1:** on the corpus, default renders have lower mean CIEDE2000 to the NegPy reference than the negadoctor reference does, and grey-card neutrality error is under dE 4. The CLI converts a 24 MP frame in under 2 s on a laptop CPU.

### Phase 2: calibration and roll (target: 12 weeks)

- [ ] `negg-calib`: flat field, bare-light crosstalk, and the camera-to-density fit from step 2. `negg-cli profile rig --flat f.dng --wedge w.dng --densities wedge.csv` writes `rig.toml`.
- [ ] Rig and film TOML schemas per `docs/PROFILES.md`, each with `source`, `measured_on`, `method`, `residual_rms`.
- [ ] Neutral-patch gamma fit (step 4, N patches) and `negg-cli profile film` that derives a film profile from a roll with a grey card frame.
- [ ] Roll estimator: pooled Dmin, gammas and cast-removal fit with outlier rejection; per-frame lock and override; `RollSettings` inheritance.
- [ ] First measured film profiles from the corpus (at least the four stocks), committed with provenance.
- [ ] Eval additions: roll-to-roll variance metric and a "three lights, one roll" neutrality test.

**Gate 2:** grey-card neutrality under dE 2 on the same roll scanned under three different light sources with a rig profile; roll-level Dmin variance under 0.02 log-D across inlier frames.

### Phase 3: desktop app (target: 16 weeks)

- [ ] `negg-ffi`: C ABI via cbindgen for open roll, render preview tile, get stats, set settings, export. No pixel buffers cross as JSON.
- [ ] Tauri 2 app, React + TypeScript + Vite. The shell never touches pixel data: it requests RGBA8 tiles of the visible region at the current zoom over `tauri::ipc::Response` (binary body with a small fixed header: magic, version, width, height, tile x/y, byte offset), and thumbnails as JPEG. Study DarkSlide's `DSRIPC01` header for the shape; write our own.
- [ ] Screens: roll browser (thumbnail grid, roll settings panel), single frame (preview, crop/rotate, the physical sliders: density, grade, three CMY offsets, toe/shoulder, black point compensation toggle), scopes (histogram, RGB parade, vectorscope computed in Rust), export dialog, batch.
- [ ] Sidecars written on every change; undo via settings history (settings are immutable and hashed).
- [ ] Packaging: macOS universal, Windows, Linux AppImage; code signing later.
- [ ] Settings schema migrations tested.

**Gate 3:** a tester who has not seen the code converts a 36-frame roll end to end using only the app, with export matching the CLI byte for byte for the same settings. Public alpha.

### Phase 4: depth (ongoing after alpha)

- [x] wgpu preview kernel implementing steps 4 to 7 for the visible tile only; CI compares against the CPU path on the corpus with a tolerance of 1/4096 per channel. Done 2026-10-27 (ADR 0036): steps 3 to 7 and the sRGB display encoding in `negg-render/src/render.wgsl` behind the `gpu` feature, used by the app's tiles only; compared on synthetic images (the corpus is local-only), within 1/4096 for the positive and one 8-bit step for the tile, on Linux CI through a software Vulkan adapter. The CPU tile path also got faster (sRGB transform on every core) and the app draws a coarser level while a slider is dragged.
- [ ] Looks as data: 3D LUTs in Rec.2020 applied after the print stage, with a documented derivation per look. No hard-coded "Frontier" constants. Parked 2026-10-25: to revisit with the owner (the tone presets below cover the near-term need without a LUT stage).
- [ ] Dust and scratch: IR channel support where present, otherwise a conservative detector plus heal with manual confirmation.
- [x] ~~Linear DNG export (write tags with the `tiff` crate; do not add the AGPL `dng` crate) so Lightroom and Capture One users can adopt the engine.~~ Dropped by the owner on 2026-10-25: the engine is not aimed at Lightroom and Capture One users, so a DNG output has no audience. The AGPL `dng` crate stays forbidden.
- [x] Tone presets, saturation and web export (spec `docs/superpowers/specs/2026-10-25-presets-and-web-export-design.md`): `print.saturation` (ADR 0033, MATHS step 7) and `print.preset` in settings schema 5; built-in presets Standard, Soft, Hard, Flat, Rich and Linear with provisional values (ADR 0034) and user presets as `*.preset.negg.toml`; `--preset`, `--preset-all-frames`, `negg preset list`, `--web` and `--web-only` in the CLI; the app's saturation slider, preset strip, Save as preset, roll preset picker and Original / Web export; web copies of 2048 px, JPEG 85, sRGB (ADR 0035, MATHS step 8); `negg_export_web` and C ABI version 2.
- [ ] Slide film (positive) mode.
- [ ] Python bindings via PyO3 so NegPy or notebooks can call the engine.

**Gate 4 (1.0):** blind side-by-side against NLP on the corpus where a small panel cannot reliably pick the NLP render.

## Explicitly out of scope until after 1.0

Scanner control (SANE, Coolscan), tethered capture, HDR bracket merge, trichrome, stitching, local masks beyond a linear gradient, cloud anything.

## Dependencies (pinned in Cargo.toml, checked by cargo deny)

| Need | Crate | Licence |
| --- | --- | --- |
| RAW decode | `rawler` 0.8 | LGPL-2.1 (dependency only) |
| TIFF read/write | `tiff` 0.11 | MIT |
| JPEG | `image` 0.25 or `jpeg-encoder` | MIT/Apache-2.0 |
| Colour management | `lcms2` 6.x | MIT (wraps Little-CMS, MIT) |
| Linear algebra for fits | `nalgebra` | Apache-2.0 |
| Parallelism | `rayon` | MIT/Apache-2.0 |
| Serialisation | `serde`, `toml`, `serde_json` | MIT/Apache-2.0 |
| CLI | `clap` | MIT/Apache-2.0 |
| GPU (phase 4) | `wgpu` | MIT/Apache-2.0 |
| Desktop (phase 3) | Tauri 2.x | MIT/Apache-2.0 |
| Property tests | `proptest` | MIT/Apache-2.0 |

Forbidden: anything GPL or AGPL, including the `dng` crate and LibRaw statically linked.

## Working conventions for Claude Code

- Before starting a task, restate it in one line and name the files you will touch. After, run the full test suite and `negg-cli eval` if the engine changed, and paste the metric deltas into the PR description.
- Every public function in `negg-render` and `negg-film` gets a doc comment linking to the step number in `docs/MATHS.md`.
- Never silently change a default constant. Change it in one place, note why in an ADR, and show the eval delta.
- When a corpus frame renders badly, add it to `eval.toml` as a tracked case before fixing it, so the fix is measured.
- If you find yourself writing the same per-pixel maths in a second language or a second crate, stop. It belongs in `negg-render`.
- Ask when: a licence is unclear, a constant has no source, the plan and the code disagree, or a gate cannot be met with the current corpus.

## Decisions already made (do not reopen without an ADR)

1. **Licence:** MPL-2.0 for every engine crate (`negg-types`, `negg-decode`, `negg-calib`, `negg-film`, `negg-render`, `negg-cms`, `negg-io`). Apache-2.0 for `negg-cli`, `negg-ffi`, the Tauri app, docs, and the profile and corpus data. Put the correct `LICENSE` file in each crate directory and set `license` in each `Cargo.toml` accordingly. Add SPDX headers to source files.
2. **Working space:** linear Rec.2020, D65. Not ProPhoto.
3. **RAW decode:** rawler as a plain dependency in all builds, LGPL-2.1 noted in `NOTICE`. No LibRaw unless a corpus frame cannot be decoded; if that happens, open an ADR before adding it.
4. **Name:** the product and CLI binary are `negg`; crates are `negg-*`; sidecars are `*.negg.json`; rig and film profiles are `*.negg.toml`. The `negg` name is confirmed free on crates.io (checked 2026-10-20; only the unrelated `unegg` exists). The repository lives under Tom's personal GitHub account, not an org; reserve the `negg` crate name on crates.io with the first CLI publish.
