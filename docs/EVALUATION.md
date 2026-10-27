# Evaluation

`negg eval` decodes every frame in `corpus/`, renders it through the engine
with default settings and scores the render: CIEDE2000 against each reference
render (mean, p95, max; on a shared block-median grid when the sizes differ),
grey-card neutrality (`(|a*| + |b*|) / 2` on the card patch of the render)
and its signed counterpart `neutrality.de = sqrt(mean(a*)^2 + mean(b*)^2)`,
the chroma of the patch's mean colour), and per-roll variance of the
estimated Dmin and gamma per channel (present when a roll has two or more
evaluated frames). When a frame has both a negadoctor and a NegPy reference,
their distance to each other is recorded as `reference_distance`. The summary
gives the mean CIEDE2000 per tool (`de2000_mean_by_tool`) and the
Gate 1 criteria under `gate1`: `engine_vs_negpy` and `negadoctor_vs_negpy`
(both over the frames that have both references), `neutrality_de_mean`, and
`pass`, which is present when both distances are and is true when the engine
is closer to NegPy than negadoctor is and the mean neutrality error is under
dE 4; fields with no data are null. Each frame's estimates are written into
the results. It prints a table and writes `eval-results.json`. Corpus frames are local-only, so entries whose files are
absent are skipped and counted, and CI evaluates zero frames until the corpus
is shared (docs/ADR/0005). CI runs it on every pull request with `--baseline
eval-baseline.json --tolerances eval.toml` and fails (exit status 3) when any
metric exceeds its baseline by more than its tolerance. To accept a new
baseline, copy a fresh `eval-results.json` over `eval-baseline.json` in a
reviewed commit that explains the delta. When a frame renders badly, add it as
a `[[case]]` in `eval.toml` first, so the fix is measured and cannot regress.

The committed corpus has no frames: frames and reference renders are
local-only (docs/ADR/0005). Personal scans that must not be published go in a
private corpus outside the repository, with the licence `LicenseRef-Private`,
and are evaluated with their own `--corpus`, `--out` and `--baseline` (see
`corpus/README.md`). A frame's manifest `geometry` can carry a crop, a buffer,
a turn and a flip but not yet an `analysis_area`.
