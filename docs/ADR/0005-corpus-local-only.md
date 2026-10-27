# 0005: corpus is local-only

## Status

Accepted, 2026-10-20.

## Context

Gate 0 in `ROADMAP.md` needs at least 10 corpus frames with reference renders, and ground rule 7 says `negg eval` runs in CI on every PR. Phase 0 put the corpus on Git LFS: `.gitattributes` patterns, a pre-push hook and `lfs: true` on the eval job's checkout. Git LFS is not installed on the development machine, it adds hooks and a bandwidth quota to the repository, and the project has one contributor, so it was costing effort without sharing anything.

## Decision

Keep the corpus local-only until a second contributor needs it. Ignore `corpus/frames/` and `corpus/reference/` in git and track only `corpus/manifest.toml` and `corpus/README.md`. Make `negg eval` skip any manifest entry whose file is absent, print the count and list the skipped entries in `eval-results.json`. Revisit this ADR when a second contributor needs the frames.

## Consequences

- CI checks out no frames, so it evaluates zero frames and the regression gate only bites locally. Contributors with frames run `cargo run -p negg-cli -- eval` and paste the table into the PR, as the working conventions already ask.
- The committed `eval-baseline.json` is produced locally and updated deliberately in a reviewed commit; CI cannot regenerate it.
- A tracked case in `eval.toml` whose frame or reference render was skipped reports `SKIPPED` and does not fail the gate. A frame that is neither evaluated nor listed as skipped is still `MISSING` and fails.
- Nothing in the eval design changes. The results schema stays at version 1 with additive fields: `corpus.evaluated`, `corpus.skipped_frames`, `corpus.skipped_references` and a top-level `skipped` list of `{ id, reason }` with relative paths only.
- The `.gitattributes` LFS patterns, the pre-push hook and the repository's LFS configuration are removed. Corpus files have no off-machine copy until the corpus is shared, so the maintainer backs them up.

## Eval delta

None, no render change.
