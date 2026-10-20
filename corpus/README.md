# negg test corpus

This directory holds the RAW film scans, reference renders and manifest that
`negg eval` uses to measure the converter. Every frame is rendered with default
settings and scored on three things: CIEDE2000 against the reference renders
where present, grey-card neutrality (mean |a*| and |b*| of the card patch), and
per-roll variance of the estimated Dmin and gammas. The corpus is what keeps
changes to the pipeline honest, so breadth matters more than polish.

## Target

30 or more frames across at least 4 film stocks, 3 scanning cameras and 2 light
sources, with a grey card in frame wherever possible. Phase 0 is gated on the
corpus holding at least 10 frames with reference renders.

## Layout

```
corpus/
  manifest.toml        one [[frame]] entry per scan (see the field notes inside)
  frames/<roll>/       RAW scans, one directory per roll
  reference/           reference renders, <id>.<tool>.tif
  LICENSE              Apache-2.0, the licence for the corpus data
```

Large files under `frames/` and `reference/` are stored with git-lfs; the
patterns live in `.gitattributes` at the repository root.

## Naming

- A roll is `rNNN` (`r001`, `r002`, ...). All frames of one roll share film
  stock, camera and light source.
- A frame id is `<roll>-<frame>` with a two-digit frame number: `r001-03`.
- The RAW is `frames/<roll>/<id>.<ext>`, keeping the camera's own extension
  in lower case: `frames/r001/r001-03.dng`.
- A reference render is `reference/<id>.<tool>.tif`, where tool is `negpy` or
  `negadoctor`: `reference/r001-03.negpy.tif`.

## Adding a frame

1. Install git-lfs (`git lfs install` once per machine) so the RAW is stored as
   an LFS object rather than committed into git history.
2. Place the RAW under `frames/<roll>/` following the naming above. Start a new
   roll directory if the stock, camera or light differs from existing rolls.
3. Add a `[[frame]]` entry to `manifest.toml`. The commented example at the top
   of that file is a template; every field is documented there. If the frame
   contains a grey card, set `has_grey_card = true` and give the card patch as
   fractions of the image size in `grey_card`.
4. Run `cargo run -p negg-cli -- eval` from the repository root and check that
   the new frame renders and scores.
5. Open a pull request. CI runs the eval on every PR.

## What makes a good frame

- A grey card (or other known neutral) in the frame, not touching the edge.
- The unexposed film border visible on at least one side, so Dmin can be read.
- No crop: the full scan as the camera captured it, including the film rebate.
- Ordinary subjects and lighting. Difficult frames are welcome too, but the
  corpus needs a baseline of normal exposures first.
- Several frames from the same roll, so per-roll variance means something.

## Reference renders

Reference renders are produced outside this repository, by darktable
negadoctor (auto pickers, default settings) and by NegPy (defaults), saved as
16-bit TIFF and named `reference/<id>.<tool>.tif`. Reference TIFFs must be
sRGB-encoded (8 or 16 bit), because the phase 0 metrics assume sRGB encoding
when they convert a render to Lab. Record the tool version in the `references`
entry so a later regeneration can be told apart.

## Licensing

Corpus data is Apache-2.0 (see `LICENSE`). A contributor may instead dedicate a
frame to the public domain under CC0-1.0. Either way, the `licence` field in
the frame's manifest entry must say which, and by setting it the contributor
affirms that they own the frame and have the right to license it. Reference
renders of a frame carry the same licence as the frame.
