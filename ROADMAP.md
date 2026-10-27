# negg roadmap

An open-source, standalone film negative converter: a Rust engine (`negg`) with a CLI first and a Tauri 2 desktop app second. Target: match or beat Negative Lab Pro on camera-scanned C-41 negatives without Lightroom.

The model itself is in `docs/MATHS.md`; decisions that shape it are in `docs/ADR/`.

## Ground rules

1. **Clean-room only.** Do not open, read, or paste from darktable (`negadoctor.c`), RawTherapee (`filmnegativeproc.cc`), NegPy, or FreeCCR. They are GPL/AGPL. The maths is written out in `docs/MATHS.md`. DarkSlide is MIT and may be consulted for its binary IPC format only.
2. **Licence.** MPL-2.0 for engine crates, Apache-2.0 for CLI, FFI, app and data (see Decisions). Every dependency must be MIT, Apache-2.0, BSD, MPL-2.0 or LGPL (LGPL linked as a normal crate dependency, never vendored into our source). No GPL, no AGPL. `cargo deny` enforces this.
3. **One pipeline.** Every pixel operation lives in `negg-render` as a pure function on a buffer plus a settings struct. No second copy in TypeScript or anywhere else. The one exception is the GPU preview kernel (ADR 0036), which is tested against the CPU path on every CI run.
4. **Float32 end to end.** Pixel data is `f32` linear from decode to the final encode. Quantisation happens exactly once, in the exporter. Analysis never goes through an 8-bit buffer.
5. **Statistics are robust.** Film base, Dmax and neutral estimation use percentiles on a block-median grid, never raw min/max or a plain mean of a rectangle. Dust and scratches must not move the result.
6. **Measured, not guessed.** No per-film-stock magic constants typed from memory. A film or rig profile is a TOML file with a `source` field saying how it was measured. A number without a measurement is not shipped; it gets a `TODO(measure)` and the no-profile fallback.
7. **Evaluation before features.** Any change to the engine runs `negg eval` in CI and must not regress the corpus metrics.
8. **Small PRs, conventional commits, tests with every change.**
9. British English in docs and UI strings; no em dashes anywhere.

Two working habits follow from these: never change a default constant silently (change it in one place, write an ADR, show the eval delta), and add a badly rendered corpus frame to `eval.toml` as a tracked case before fixing it, so the fix is measured.

## Milestones

| Phase | What | Status |
| --- | --- | --- |
| 0 | Corpus, eval harness, workspace and CI | Built |
| 1 | Core engine and CLI: decode, film base, inversion, print curve, auto colour, colour management, export | Built |
| 2 | Calibration and roll: rig and film profiles, roll pooling, locks | Built |
| 3 | Desktop app and C ABI | Built; hand testing open |
| 4 | Depth (ongoing after alpha) | In progress |

Each phase has a gate. None is claimed met yet: the committed corpus has no frames and there is no rig, so Gates 0 to 2 cannot be shown by data.

- **Gate 0:** eval runs in CI on every PR and the corpus has at least 10 frames with references.
- **Gate 1:** on the corpus, default renders have lower mean CIEDE2000 to the NegPy reference than the negadoctor reference does, and grey-card neutrality error is under dE 4. The CLI converts a 24 MP frame in under 2 s on a laptop CPU.
- **Gate 2:** grey-card neutrality under dE 2 on the same roll scanned under three different light sources with a rig profile; roll-level Dmin variance under 0.02 log-D across inlier frames.
- **Gate 3:** a tester who has not seen the code converts a 36-frame roll end to end using only the app, with export matching the CLI byte for byte for the same settings. Public alpha. The byte-for-byte half is a test (`crates/negg-cli/tests/app_parity.rs`, ADR 0024); the hand test is open.
- **Gate 4 (1.0):** blind side-by-side against NLP on the corpus where a small panel cannot reliably pick the NLP render.

## Phase 4

Done:

- GPU preview kernel for the visible tiles (ADR 0036), compared with the CPU path in CI.
- Tone presets, saturation and web export (ADRs 0033 to 0035).

Open or parked:

- **Looks as data** (3D LUTs in Rec.2020 after the print stage, such as a scanner-style look): parked, to revisit.
- **Bulk analysis area** (select several frames, give them one analysis area): parked, to revisit.
- **Dust and scratch** (IR channel where present, otherwise a conservative detector plus heal with manual confirmation): parked; a simple heal brush may cover the need.
- **Python bindings:** not planned; a contributed PR is welcome.
- **Slide film (positive) mode:** not planned.
- **Linear DNG export:** dropped. negg is not aimed at Lightroom and Capture One users. The AGPL `dng` crate stays forbidden.
- Arbitrary-angle straightening.

## Out of scope until after 1.0

Scanner control (SANE, Coolscan), tethered capture, HDR bracket merge, trichrome, stitching, local masks beyond a linear gradient, cloud anything.

## Dependencies

Pinned in the workspace `Cargo.toml` and checked by `cargo deny`.

| Need | Crate | Licence |
| --- | --- | --- |
| RAW decode | `rawler` | LGPL-2.1 (dependency only) |
| TIFF read/write | `tiff` | MIT |
| JPEG | `image` | MIT/Apache-2.0 |
| Colour management | `lcms2` | MIT (wraps Little-CMS, MIT) |
| Linear algebra for fits | `nalgebra` | Apache-2.0 |
| Parallelism | `rayon` | MIT/Apache-2.0 |
| Serialisation | `serde`, `toml`, `serde_json` | MIT/Apache-2.0 |
| CLI | `clap` | MIT/Apache-2.0 |
| GPU preview | `wgpu` | MIT/Apache-2.0 |
| Desktop | Tauri 2.x | MIT/Apache-2.0 |
| Property tests | `proptest` | MIT/Apache-2.0 |

Forbidden: anything GPL or AGPL, including the `dng` crate and LibRaw statically linked.

## Decisions

Reopening any of these needs an ADR.

1. **Licence:** MPL-2.0 for every engine crate (`negg-types`, `negg-decode`, `negg-calib`, `negg-film`, `negg-render`, `negg-cms`, `negg-io`). Apache-2.0 for `negg-session`, `negg-cli`, `negg-ffi`, the Tauri app, docs, and the profile and corpus data. Each crate has its `LICENSE` file and `license` field, and source files carry SPDX headers (ADR 0001).
2. **Working space:** linear Rec.2020, D65. Not ProPhoto (ADR 0002).
3. **RAW decode:** rawler as a plain dependency in all builds, LGPL-2.1 noted in `NOTICE`. No LibRaw unless a corpus frame cannot be decoded, and then only with an ADR (ADR 0003).
4. **Name:** the product and CLI binary are `negg`; crates are `negg-*`; sidecars are `*.negg.json`; rig and film profiles are `*.negg.toml` (ADR 0004).
