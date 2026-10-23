# negg test corpus

The manifest here lists the RAW film scans and reference renders that
`negg eval` renders with default settings and scores: CIEDE2000 against the
reference renders where present, grey-card neutrality (mean |a*| and |b*| of
the card patch), and per-roll spread of the estimated Dmin and gammas. Each
roll is rendered in roll mode, with its rig and film profile when the manifest
names them, and the Gate 2 metrics (docs/ADR/0019) are reported: the standard
deviation of the per-frame Dmin over each roll's inliers, grey-card neutrality
per light source on a film roll scanned under three lights with a rig, and the
spread of the pooled values between rolls of one stock. The corpus keeps
pipeline changes honest, so breadth matters more than polish.

## Target

30 or more frames across at least 4 film stocks, 3 scanning cameras and 2 light
sources, with a grey card in frame wherever possible. Phase 0 is gated on the
corpus holding at least 10 frames with reference renders.

## Layout and storage

```
corpus/
  manifest.toml        one [[frame]] entry per scan (see the field notes inside)
  frames/<roll>/       RAW scans, one directory per roll (local-only)
  reference/           reference renders, <id>.<tool>.tif (local-only)
  LICENSE              Apache-2.0, the licence for the corpus data
```

Frames and reference renders are local-only (docs/ADR/0005): they live under
`corpus/frames/` and `corpus/reference/` on each maintainer's machine and are
ignored by git; only `manifest.toml` and this README are tracked. `negg eval`
skips any manifest entry whose file is not present locally, prints how many it
skipped and lists them under `skipped` in `eval-results.json`, so CI (which has
no frames) runs the plumbing against zero frames. The corpus moves to shared
storage when a second contributor needs it.

## Naming

- A roll is `rNNN` (`r001`, `r002`, ...). All frames of one roll share film
  stock, camera and light source.
- A frame id is `<roll>-<frame>` with a two-digit frame number: `r001-03`.
- The RAW is `frames/<roll>/<id>.<ext>`, keeping the camera's own extension
  in lower case: `frames/r001/r001-03.dng`.
- A reference render is `reference/<id>.<tool>.tif`, where tool is `negpy` or
  `negadoctor`: `reference/r001-03.negpy.tif`.
- A physical film roll scanned more than once (for example under three light
  sources for Gate 2) is one `roll` per scan, all sharing a `film_roll` such
  as `f001`. Name the rig profile each scan used in `rig`.

## Adding a frame

1. Place the RAW under `frames/<roll>/` following the naming above. Start a new
   roll directory if the stock, camera or light differs from existing rolls.
2. Add a `[[frame]]` entry to `manifest.toml`. The commented example at the top
   of that file is a template; every field is documented there. If the frame
   contains a grey card, set `has_grey_card = true` and give the card patch as
   fractions of the image size in `grey_card`.
3. Run `cargo run -p negg-cli -- eval` from the repository root and check that
   the new frame renders and scores and that the skipped count is as expected.
4. Open a pull request with the manifest change and paste the eval table into
   the description. CI sees zero frames, so the pasted table is the evidence.

## What makes a good frame

- A grey card (or other known neutral) in the frame, not touching the edge.
- The unexposed film border visible on at least one side, so Dmin can be read.
- No crop: the full scan as the camera captured it, including the film rebate.
- Ordinary subjects and lighting first; difficult frames are welcome later.
- Several frames from the same roll, so per-roll variance means something.
- For Gate 2: one roll of film with a grey card, scanned with a rig profile
  under at least three light sources (`film_roll` plus `rig`), and at least
  two rolls of each stock.

## Reference renders

Reference renders are produced outside this repository, by darktable
negadoctor (auto pickers, default settings) and by NegPy (defaults), saved as
16-bit TIFF and named `reference/<id>.<tool>.tif`. Reference TIFFs must be
sRGB-encoded (8 or 16 bit), because the phase 0 metrics assume sRGB encoding
when they convert a render to Lab. Record the tool version in the `references`
entry so a later regeneration can be told apart.

## Private corpus

Frames you can use for testing but do not want to license or publish (personal
photos, for example) go in a private corpus outside this repository, never
under `corpus/`. A private corpus has the same layout (its own
`manifest.toml`, `frames/`, `reference/`) and its frames use
`licence = "LicenseRef-Private"`. Run the eval against it with its own output
and baseline, so nothing derived from it lands in a tracked file:

```
negg eval --corpus ~/negg-private-corpus \
  --out ~/negg-private-corpus/eval-results.json \
  --baseline ~/negg-private-corpus/eval-baseline.json
```

Never commit a private frame's manifest entry, a reference render, a crop or a
test fixture made from one, or an `eval-baseline.json` regenerated from it. A
test fails if the committed manifest carries `LicenseRef-Private`.

## Licensing

Corpus data is Apache-2.0 (see `LICENSE`). A contributor may instead dedicate a
frame to the public domain under CC0-1.0. Either way, the `licence` field in
the frame's manifest entry must say which, and by setting it the contributor
affirms that they own the frame and have the right to license it. Reference
renders of a frame carry the same licence as the frame.
