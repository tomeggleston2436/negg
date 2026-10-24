# 0025: frontend state and tests, and the npm licence check

## Status

Proposed, 2026-10-24 (Rulings 84 and 85).

## Context

The frontend is new code in a language the project has not used. The pipeline rule forbids pixel maths outside `negg-render`, so the frontend only moves what Rust returns. Tauri's `tauri-driver` has no macOS support, so a WebDriver smoke test cannot run on the maintainer's platform. `cargo deny` checks Rust licences only, but the project ships npm dependencies too.

## Decision

**Frontend (Ruling 84).** React 19 function components. State lives in one `useReducer` store with React context, with no state library. Server state (roll, frame views, history) is whatever the last command returned. Tile canvases are drawn with `putImageData` from the binary response (ADR 0022).

**Tests (Ruling 84).** vitest in the node environment for pure modules: tile and scopes parsers, tile grid maths, the slider table and its clamping (ADR 0023), the reducer, and fixture decoders. No DOM and no WebDriver: the UI is not smoke-tested in CI. The Rust command functions are tested without a webview.

**npm licence check (Ruling 85).** `app/scripts/check-licences.mjs` runs `pnpm licenses list --json` over all dependencies, dev included, and fails listing every package whose licence is not in the `deny.toml` allow list: MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, ISC, 0BSD, MPL-2.0, Zlib, Unicode-3.0, CC0-1.0, LGPL variants, BSL-1.0, Apache-2.0 WITH LLVM-exception and Unicode-DFS-2016. An `OR` expression passes when one operand is allowed. The CI `app` job runs it.

## Consequences

- **UI behaviour is untested end to end.** Rendering, layout and event wiring rely on manual use and on the pure-module tests. Cost of choosing wrongly: add jsdom and testing-library later.
- **No state library to audit or update.** The cost is a hand-written reducer.
- **Licences.** The npm tree is held to the same allow list as the Rust tree, so a new dependency with a disallowed licence fails CI. Cost of choosing wrongly: one allow-list line.
- **No maths in TypeScript.** The frontend tests cover layout arithmetic (tile grids, clamping), which is not pixel maths.

## Eval delta

None, no render change.
