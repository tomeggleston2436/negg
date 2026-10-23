# profiles

Committed rig and film profiles for `negg`. This directory is empty of
profiles on purpose: no measured profile is committed until frames and a rig
exist to measure (Ruling 54, ADR 0019). Nothing here is a typical value for
any stock or camera.

## Layout

```
profiles/rig/<name>.rig.negg.toml
profiles/film/<name>.film.negg.toml
```

The `*.negg.toml` suffix is fixed by decision 4 (`docs/ADR/0004-naming.md`).
Profiles are found only by explicit path (`--rig FILE`, `--film FILE`, or
`Settings.profiles`); there is no search path (Ruling 46), so this directory
is a place to keep them, not a lookup location. A rig profile with a
`[flat_field]` table also needs its flat frame at the path stored in that
table (relative to the profile file) alongside it; `profiles_dir.rs` does
not check that the frame is there.

## Provenance rules

The full rules and the schema are in `docs/PROFILES.md`. In short:

- Every numeric value needs a `source` saying how it was measured.
- No stock-specific constant is committed without a measurement.
- A `source` that is empty or contains `TODO(measure)` is rejected by the
  readers, so a placeholder cannot be loaded by mistake.
- `residual_rms` comes from the tool that made the fit, never by hand.
- Changing a committed profile is a re-measurement; the commit says what
  was re-measured and the `negg-cli eval` delta goes in the pull request.

## Producing a profile

```
negg profile rig  ... --out profiles/rig/<name>.rig.negg.toml
negg profile film ... --out profiles/film/<name>.film.negg.toml
```

See `negg profile rig --help` and `negg profile film --help` for the inputs
(`wedge.csv` and `patches.csv` formats are in `docs/PROFILES.md`).

`crates/negg-io/tests/profiles_dir.rs` loads and validates every
`*.negg.toml` committed here; with no files it passes.

## Licence

Apache-2.0, see `LICENSE`. Profile data committed here is also Apache-2.0
(decision 1).
