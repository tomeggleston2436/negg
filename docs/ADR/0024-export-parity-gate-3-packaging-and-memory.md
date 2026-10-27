# 0024: export parity with the CLI, the Gate 3 test, packaging and memory

## Status

Accepted, 2026-10-24 (amended below). Gate 3 itself stays open (see Decision).

## Context

Gate 3 asks that a tester can convert a 36-frame roll in the app, and that the app's output is what the CLI would produce. The corpus has no frames and there is no tester in phase 3, so the usability half of the gate cannot be met by data. The app also has to ship as installable packages without notarisation or signing certificates the project does not have, and a 36-frame roll in memory needs a stated budget.

## Decision

**Parity by construction.** App exports call the CLI's own functions with CLI-equivalent requests and no extra overrides. A whole roll is `batch_dir(dir, req)`, which equals `negg batch <dir> --format F [--out O]`. One or several frames are `convert_one(frame, req)` per frame, which equals `negg convert <frame> --format F [--out O]`, after `flush` has written the current roll sidecar and pool. The output space is a setting in the roll panel; the export dialog commits it as a roll edit instead of passing `--space`. Export therefore re-decodes and re-pools from disk and never reads the preview caches. Before it flushes, `Session::export` checks every sidecar it may write against what the session last read or wrote (a changed modification time or length and changed bytes): a frame whose sidecar changed turns read-only with its overrides read again from disk, a changed `roll.negg.json` is reported, and nothing is exported, so the user sees what the export would use and can export again. A pending re-pool runs first (ADR 0023).

**Gate 3 automation.** `crates/negg-cli/tests/app_parity.rs` drives `Session` on a synthetic 6-frame float TIFF roll through roll and frame edits (grade, density, CMY, toe, shoulder, BPC, rotate 90, flip, roll crop, a frame crop override, analysis buffer, lock, undo, redo), exports TIFF16 and JPEG through `Session::export`, then runs the `negg` binary's `batch` and `convert` on the same directory into other output directories and asserts byte equality of every output (no masking) and of every sidecar. It also checks level-0 tile consistency (ADR 0022) and that the estimates in `frame_view` equal those in the export sidecar. Byte equality without masking depends on the deterministic ICC header of ADR 0022. A second test, `crates/negg-cli/tests/analysis_area.rs`, sets the roll's analysis area through the session before pass 1 and compares the session's own flushed sidecars and views with `negg batch --analysis-area` on the same directory, like `app_parity.rs` (a held frame with the area on its film gives the clean frame's estimates while the export keeps the whole frame, ADR 0021). The app's own command layer is tested without a webview in `app/src-tauri/tests`, and the frontend's wire fixtures there are the real serde output of sample values (`NEGG_BLESS=1` rewrites them, as for the C header).

Human only, and therefore open: the 36-frame roll by a tester who has not seen the code, real RAW frames, perceived preview quality, and installing the packages on each OS. Gate 3 stays open until that run happens. The automated test proves the byte rule, not the usability rule.

**Packaging.** `pnpm tauri build` via `@tauri-apps/cli` (`cargo-tauri` is neither installed nor needed). Local builds on arm64 macOS use `--target aarch64-apple-darwin`, ad-hoc signed (`"signingIdentity": "-"`), without notarisation; a universal build needs the x86_64 Rust target and is done in CI. `.github/workflows/package.yml` runs on `workflow_dispatch` and `v*` tags only, never on pull requests: macos-14 `--target universal-apple-darwin` (builds the .app and the .dmg; the .dmg is uploaded, because a bare .app directory would lose its permissions and symlinks in the artifact zip), windows-latest (NSIS .exe), ubuntu-22.04 (AppImage). Node 22 and pnpm 12.14.0 (through Corepack, the version that wrote `pnpm-lock.yaml`) are pinned, as in the CI `app` job. It uploads artifacts and publishes nothing. Identifier `io.github.tomeggleston2436.negg`, product name `negg`. Deferred: Developer ID signing and notarisation, Authenticode, an updater, and Linux .deb and .rpm.

**Memory.** Per frame the session caches the `AnalysisGrid` (block medians and the coefficient of variation) and the density its pool input needs, about 20 MB at 1024 px and about 720 MB for 36 frames, plus the render plan. The budget stays, and the duplicate per-frame density is gone: pass 1 does not keep the whole-grid density, which is the pool input's own density when the frame has no analysis area and no crop and is rebuilt from the grid when a plan needs it otherwise. Only the open frame keeps level 0 and its pyramid (about 390 MB at 24 MP). Pass 1 decodes one frame at a time.

## Consequences

- **Cost of parity.** An export decodes again, so it is slower than rendering from cache; correctness comes first.
- **Interaction with edits.** A pending edit not yet flushed would be missed by the CLI path, so `flush` runs before every export.
- **Unsigned packages.** Users on macOS must allow the app manually; Windows shows SmartScreen. Cost of choosing wrongly: workflow edits.
- **Memory.** About 1.1 GB peak for a 36-frame roll with one frame open. Cost of choosing wrongly: evict the grids of off-screen frames and re-decode on demand.
- **Memory during an export.** The export runs the CLI's flows, which decode every frame again, so before they start the session drops the open frame (its level 0 and pyramid, about 390 MB at 24 MP) and its cached plans. The expected peak of a whole-roll export of 36 frames at 24 MP is then the session's grids (about 720 MB), plus the batch flow's own pass 1 analyses (one grid density per frame, about 8 MB each, about 300 MB), plus one frame in pass 2 (its decoded linear image and its render, about 290 MB each in `f32`, the 16-bit output and its compressed strips): about 1.8 GB, against about 2.2 GB with the open frame kept. Estimated from the buffer sizes, not measured: `TODO(measure)` on a real 36-frame roll.
- **Pre-analysis decodes.** The negative thumbnails and tiles shown before Analyse decode a frame once; its analysis grid (with its size, step 7 matrix and decoder notes) is kept as that frame's pass 1 data, and pass 1 uses it instead of decoding the frame again when the profiles have not changed since. The memory held between the preview and Analyse is bounded by the same per-frame grids as above.
- **Gate 3.** Not claimed met by this work.

## Eval delta

None, no render change.
