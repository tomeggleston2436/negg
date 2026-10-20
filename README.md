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
