# 0004: naming

## Status

Accepted, 2026-10-20. Decision 4 in `ROADMAP.md`.

## Context

The product, the CLI binary, the crates, the sidecar files and the profile files all need names before phase 0 lands, and the crate name must be free on crates.io so the CLI can be published.

## Decision

The product and the CLI binary are `negg`. Crates are `negg-*`. Sidecars are `*.negg.json`. Rig and film profiles are `*.negg.toml`. The repository lives under Tom's personal GitHub account, not an organisation. Reserve the `negg` crate name on crates.io with the first CLI publish.

## Consequences

- The `negg` name was confirmed free on crates.io on 2026-10-20; only the unrelated `unegg` exists. Publish early to keep it.
- `docs/PROFILES.md` uses the `*.negg.toml` suffix for both profile kinds and `docs/MATHS.md` step 8 uses `frame.negg.json` for the sidecar.
- Every crate directory under `crates/` is named `negg-<role>` and each has a `README.md` stating what it owns and must not do.
- Renaming anything here after phase 1 breaks sidecars and profiles on disk, so it needs a new ADR and a `schema_version` bump.

## Eval delta

None, no render change.
