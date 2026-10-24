# 0024: export parity with the CLI, the Gate 3 test, packaging and memory

## Status

Proposed, 2026-10-24 (Rulings 82, 83, 86 and 87).

## Context

Gate 3 asks that a tester can convert a 36-frame roll in the app, and that the app's output is what the CLI would produce. The corpus has no frames (Ruling 66) and there is no tester in phase 3, so the usability half of the gate cannot be met by data. The app also has to ship as installable packages without notarisation or signing certificates the project does not have, and a 36-frame roll in memory needs a stated budget.

## Decision

**Parity by construction (Ruling 82).** App exports call the CLI's own functions with CLI-equivalent requests and no extra overrides. A whole roll is `batch_dir(dir, req)`, which equals `negg batch <dir> --format F [--out O]`. One or several frames are `convert_one(frame, req)` per frame, which equals `negg convert <frame> --format F [--out O]`, after `flush` has written the current roll sidecar and pool. The output space is a setting in the roll panel; the export dialog commits it as a roll edit instead of passing `--space`. Export therefore re-decodes and re-pools from disk and never reads the preview caches.

**Gate 3 automation (Ruling 83).** `crates/negg-cli/tests/app_parity.rs` drives `Session` on a synthetic 6-frame float TIFF roll through roll and frame edits (grade, density, CMY, toe, shoulder, BPC, rotate 90, flip, roll crop, a frame crop override, analysis buffer, lock, undo, redo), exports TIFF16 and JPEG through `Session::export`, then runs the `negg` binary's `batch` and `convert` on the same directory into other output directories and asserts byte equality of every output (no masking) and of every sidecar. It also checks level-0 tile consistency (ADR 0022) and that the estimates in `frame_view` equal those in the export sidecar. Byte equality without masking depends on the deterministic ICC header of ADR 0022.

Human only, and therefore open: the 36-frame roll by a tester who has not seen the code, real RAW frames, perceived preview quality, and installing the packages on each OS. Gate 3 stays open until that run happens. The automated test proves the byte rule, not the usability rule.

**Packaging (Ruling 86).** `pnpm tauri build` via `@tauri-apps/cli` (`cargo-tauri` is neither installed nor needed). Local builds on arm64 macOS use `--target aarch64-apple-darwin`, ad-hoc signed (`"signingIdentity": "-"`), without notarisation; a universal build needs the x86_64 Rust target and is done in CI. `.github/workflows/package.yml` runs on `workflow_dispatch` and `v*` tags only, never on pull requests: macos-14 `--target universal-apple-darwin` (.app, .dmg), windows-latest (NSIS .exe), ubuntu-22.04 (AppImage). It uploads artifacts and publishes nothing. Identifier `io.github.tomeggleston2436.negg`, product name `negg`. Deferred: Developer ID signing and notarisation, Authenticode, an updater, and Linux .deb and .rpm.

**Memory (Ruling 87).** Per frame the session caches the `AnalysisGrid` and its `grid_density` (about 20 MB at 1024 px, about 720 MB for 36 frames) plus the plan. Only the open frame keeps level 0 and its pyramid (about 390 MB at 24 MP). Pass 1 decodes one frame at a time.

## Consequences

- **Cost of parity.** An export decodes again, so it is slower than rendering from cache; correctness comes first.
- **Interaction with edits.** A pending edit not yet flushed would be missed by the CLI path, so `flush` runs before every export.
- **Unsigned packages.** Users on macOS must allow the app manually; Windows shows SmartScreen. Cost of choosing wrongly: workflow edits.
- **Memory.** About 1.1 GB peak for a 36-frame roll with one frame open. Cost of choosing wrongly: evict the grids of off-screen frames and re-decode on demand.
- **Gate 3.** Not claimed met by this work.

## Eval delta

None, no render change.
