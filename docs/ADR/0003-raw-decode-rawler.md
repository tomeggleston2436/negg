# 0003: RAW decode via rawler

## Status

Accepted, 2026-10-20. Decision 3 in `ROADMAP.md`.

## Context

`docs/MATHS.md` step 1 needs a RAW decoder that can run demosaic and active-area crop without applying white balance, a colour matrix or an sRGB conversion, and that exposes the camera-to-XYZ matrix and the documented white and black levels. Ground rule 2 forbids GPL and AGPL dependencies. LibRaw is the obvious alternative but its licensing (and static linking in particular) conflicts with that rule.

## Decision

Use `rawler` 0.8 as a plain crate dependency in all builds. Note its LGPL-2.1 licence in `NOTICE`. Do not add LibRaw unless a corpus frame cannot be decoded by `rawler`; if that happens, open an ADR before adding it.

## Consequences

- `rawler` is LGPL-2.1 and is linked as a normal dependency, never vendored into our source, which is what ground rule 2 allows.
- `cargo deny` allows LGPL for dependencies linked as normal crates, per ground rule 2; `rawler` is the dependency that currently relies on that allowance.
- Phase 1 integration tests check white level pinning: the same frame decoded twice is identical, and two frames of one roll share a scale.
- A camera that `rawler` cannot decode blocks that corpus frame rather than triggering an ad hoc dependency; the ADR path is the only way in for LibRaw.

## Eval delta

None, no render change.
