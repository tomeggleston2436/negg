# 0001: licensing

## Status

Accepted, 2026-10-20. Decision 1 in `ROADMAP.md`.

## Context

Ground rule 1 requires a clean room: the existing open-source negative converters are GPL or AGPL and must not be read or copied. Ground rule 2 requires that every dependency be MIT, Apache-2.0, BSD, MPL-2.0 or LGPL, with LGPL only as a normal crate dependency, never vendored. The project needs a licence split that keeps the engine protected while leaving the parts people integrate with permissive.

## Decision

Engine crates (`negg-types`, `negg-decode`, `negg-calib`, `negg-film`, `negg-render`, `negg-cms`, `negg-io`) are MPL-2.0. `negg-cli`, `negg-ffi`, the Tauri app, `docs/`, and the profile and corpus data are Apache-2.0. Put the correct `LICENSE` file in each crate directory, set `license` in each `Cargo.toml`, and add SPDX headers to source files.

## Consequences

- `cargo deny` enforces the allowed dependency licences from phase 0; GPL and AGPL are forbidden, including the `dng` crate and statically linked LibRaw.
- `rawler` (LGPL-2.1) is used only as a dependency and is noted in `NOTICE` (see 0003).
- Nobody working on `negg` opens darktable, RawTherapee, NegPy or FreeCCR sources. The maths lives in `docs/MATHS.md`.
- Profiles and corpus frames contributed to the repository are Apache-2.0, so contributors must be told this when they add frames. The repository licence for corpus data is Apache-2.0 per decision 1; a contributor may additionally dedicate their own frames to the public domain under CC0-1.0, which is compatible.

## Eval delta

None, no render change.
