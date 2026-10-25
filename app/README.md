# negg app

The negg desktop app: a Tauri 2 backend (`src-tauri`) over the `negg-session`
crate, with a React 19 and TypeScript frontend (`src`). It opens a folder of
scanned negatives as a roll, shows live previews with scopes, edits the
settings, and exports through the same functions the `negg` CLI uses, so its
files are byte-identical to `negg batch` and `negg convert` (ADR 0024).

The app is its own cargo workspace (`src-tauri/Cargo.toml`, its own
`Cargo.lock`), so `cargo build --workspace` at the repository root never builds
Tauri or needs Node (ADR 0020). It links the engine as Rust crates through
`negg-session`; it does not use the C ABI. Licence: Apache License 2.0.

## What it does

1. **Open a roll folder.** The app lists every RAW or TIFF in it, in name order.
   Opening writes nothing.
2. **Set the analysis area, then Analyse.** For a folder that has no
   `roll.negg.json` yet, the app shows the negatives as scanned and waits.
   Draw the roll's analysis area over the whole film including its clear edge
   (rebate), where the film base is measured, leaving out only the film
   holder; it does not crop the photos. If an area leaves the clear edge out,
   the app says so. Nothing is
   analysed until you press Analyse. A folder that has a `roll.negg.json`
   (opened or batched before) is analysed at once. A frame whose Dmax is at the
   density cap (the holder is in what was analysed) gets a "needs analysis
   area" badge and is left out of the roll's gamma and cast pools (ADR 0032).
3. **Edit.** The frame screen has the physical sliders (density, grade, cyan,
   magenta, yellow, toe, shoulder, black point compensation), rotate and flip,
   the output crop, the frame's own analysis area ("Use roll analysis area"
   drops it again), lock, undo and redo (Cmd or Ctrl+Z, with Shift to redo, on
   every screen; on macOS also the Edit menu), the
   scopes, and a toggle to show the negative. Arrow keys or `[` and `]` move
   between frames, `F` fits and `1` shows actual pixels. The roll panel has the
   auto colour toggle, the output space, the pooling switches and the rig and
   film profiles. A drag previews; release commits. Changes that alter what the
   roll pool sees are re-pooled in the background, with a "re-pooling"
   indicator (ADR 0023).
4. **Export.** Export this frame, the selected frames or the whole roll as
   16-bit TIFF or JPEG, with the output space chosen in the dialog, beside the
   sources or into another folder.

Sidecars are written next to the frames, as the CLI writes them: each frame's
`<stem>.negg.json` (its settings, overrides and estimates) and the roll's
`roll.negg.json`. They are saved 500 ms after the last edit, before an export
and when the window closes or the app quits. A sidecar that could not be read,
or that changed on disk behind the app (the CLI rewrote it), is never
overwritten: that frame turns read-only and the app says so (open the folder
again to see and edit what is on disk). If the edits of a roll cannot be saved
when you close, quit or open another folder, the app refuses once and says why;
the next attempt goes ahead without them. Setting an analysis area, like any
roll setting, saves `roll.negg.json` in the folder, also before Analyse; a roll
edit also brings every other frame's sidecar up to date, as `negg batch` would.
Only browsing a folder (without any edit) saves nothing.

## Requirements

- Rust 1.90 or newer (Tauri 2.12 needs it; the engine crates build on 1.89)
- Node 22 and pnpm 12.14.0, the version that wrote `pnpm-lock.yaml`. Corepack
  comes with Node: `corepack enable && corepack prepare pnpm@12.14.0 --activate`
- Linux only: the Tauri 2 system packages, for example on Ubuntu 22.04
  `sudo apt-get install -y libwebkit2gtk-4.1-dev libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev`
  (and `patchelf` and `file` to build the AppImage)

## Build and run

```sh
cd app
pnpm install --frozen-lockfile
pnpm tauri dev          # the dev server on port 1420 and the app window
pnpm build              # type check and build the frontend into dist/
```

`pnpm tauri` is `@tauri-apps/cli`; `cargo-tauri` is neither installed nor
needed. Tauri embeds `dist/` when it compiles (`generate_context!`), so run
`pnpm build` once before any `cargo` command in `src-tauri`.

## Test

```sh
cd app
node scripts/check-licences.mjs      # or: pnpm licences
pnpm exec tsc --noEmit
pnpm test                            # vitest, pure modules, no DOM
cd src-tauri
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test                           # the commands without a webview, and the IPC fixtures
```

The frontend tests cover the parsers, tile maths, slider table and reducer; the
UI itself is not tested end to end (ADR 0025). `app/src/ipc/__fixtures__` are
the real serde output of sample values: `cargo test` fails when they differ and
`NEGG_BLESS=1 cargo test` rewrites them. The licence check holds the npm tree
to the allow list of the root `deny.toml`. Licences and advisories for the Rust
side are checked from the repository root with
`cargo deny --manifest-path app/src-tauri/Cargo.toml check` (CI passes
`--config deny.toml`).

## Package

```sh
cd app
pnpm tauri build --target aarch64-apple-darwin    # Apple Silicon, ad hoc signed
```

On macOS this makes `negg.app` and a `.dmg` under
`src-tauri/target/<target>/release/bundle`. The bundles are ad hoc signed
(`"signingIdentity": "-"`) and not notarised, so macOS asks you to allow the
app manually, and Windows shows SmartScreen for the installer. The workflow
`.github/workflows/package.yml` builds a universal macOS `.dmg`, a Windows NSIS
`.exe` and a Linux AppImage; it runs only by hand (`workflow_dispatch`) or for
a `v*` tag, never on pull requests, and uploads artifacts without publishing
anything. Developer ID signing, notarisation, an updater and `.deb` and `.rpm`
packages are deferred (ADR 0024).

## Layout

- `src-tauri/src/commands.rs`: the command bodies, plain functions over
  `AppState`
- `src-tauri/src/state.rs`: the open session, the background analyse loop, the
  debounced flush, the events (`negg://analysis`, `negg://thumbs`,
  `negg://export`, `negg://error`) and the close and quit refusals
- `src-tauri/src/lib.rs`: the `#[tauri::command]` wrappers (snake_case
  arguments) and the window and menu wiring
- `src/ipc`: the typed command calls and the tile and scopes parsers (the wire
  formats of `negg-session`)
- `src/state`, `src/screens`, `src/components`: the reducer store, the roll
  browser, the frame screen, the export dialog and the controls

The frontend computes no pixel values: tiles, thumbnails and scopes are
rendered by `negg-render` and only drawn here (ADR 0025).
