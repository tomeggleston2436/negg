# negg test corpus

The manifest here lists the RAW film scans and reference renders that
`negg eval` renders with default settings and scores: CIEDE2000 against the
reference renders where present, grey-card neutrality (mean |a*| and |b*| of
the card patch), and per-roll spread of the estimated Dmin and gammas. Each
roll is rendered in roll mode, with its rig and film profile when the manifest
names them, and the Gate 2 metrics (docs/ADR/0019) are reported: the standard
deviation of the per-frame Dmin over each roll's inliers, grey-card neutrality
per light source on a film roll scanned under three lights with a rig, and the
spread of the pooled values between rolls of one stock. References exported
by hand from Negative Lab Pro are registered onto the render first and also
described by tone and colour (see "Negative Lab Pro references" below). The
corpus keeps pipeline changes honest, so breadth matters more than polish.

## Target

30 or more frames across at least 4 film stocks, 3 scanning cameras and 2 light
sources, with a grey card in frame wherever possible. Phase 0 is gated on the
corpus holding at least 10 frames with reference renders.

## Layout and storage

```
corpus/
  manifest.toml        one [[frame]] entry per scan (see the field notes inside)
  frames/<roll>/       RAW scans, one directory per roll (local-only)
  reference/           reference renders, <id>.<tool>.tif or <id>.nlp.jpg (local-only)
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
  `negadoctor`: `reference/r001-03.negpy.tif`. A Negative Lab Pro export is
  `reference/<id>.nlp.jpg` (or `.tif`). Keep the tool name in the file name:
  a tracked case whose reference file is absent is matched to its tool by it.
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

## Per-frame geometry

A frame whose scan shows the film holder, or which should be framed
differently from the rest of its roll, takes a `geometry` table in its
manifest entry, with the fields and rules of `Settings.geometry`:

```
geometry = { crop = { x = 0.04, y = 0.06, width = 0.92, height = 0.88 }, rotate_degrees = 0, flip_horizontal = false }
```

The crop is given as fractions of the scan in its decoded orientation; every
field is optional (`analysis_buffer` defaults to 0.05). `negg eval` renders
the frame with it as a frame override, so the crop restricts what the
estimators see and frames the render the references are scored against; the
roll still pools over all its frames. The grey card rect stays in fractions of
the whole scan. A geometry that `Settings.geometry` would reject (a crop
outside the frame, a turn other than 0, 90, 180 or 270) fails the manifest.

## Negative Lab Pro references

Tool `nlp` is a Negative Lab Pro export: an 8-bit sRGB JPEG (TIFF is read
too), cropped by hand, at its own size, and possibly mirrored (a film scanned
base side to the lens) or turned by quarter turns. It is never compared pixel
by pixel. `negg eval` registers it onto the render (docs/ADR/0028): it tries
the 8 orientations and, for each, a scale at which the reference covers 60%
to 110% of the render width, every placement within the frame and a small
rotation of up to 2 degrees (hand straightening in the editor), maximising
the correlation of L* (and, on the finer levels, of its edges) coarse to fine, then checks the edges (the correlation
of the L* gradient) at the best placement. The reference is resampled onto
the render's pixels and only the overlap is scored. Per reference the results
record the orientation, scale, rotation angle (`angle_degrees`), offset, overlap fraction and correlation, and
describe tone and colour: L* at the 1st, 50th and 99th percentiles (contrast
and highlight roll-off), mean C*ab (saturation) and the mean a* and b*
difference (cast), for the render and the reference. The summary averages
them per tool and overall, and `eval.toml` gates three of those means. A
reference whose edge correlation is below 0.5 is listed as `unaligned` and
left out of every mean. A NegPy or negadoctor reference whose size matches
neither the render nor its analysis grid is registered the same way, and is
skipped as a size mismatch when it does not register.

Export from Negative Lab Pro as sRGB JPEG at full quality; any crop, size or
orientation is fine. Give the frame a `geometry` crop that removes the film
holder, so the render and the export show roughly the same picture.

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

A private corpus is where Negative Lab Pro references usually live: export
each frame from NLP as `reference/<id>.nlp.jpg`, list it in the frame's
`references` with `tool = "nlp"` and the NLP version, give the frame the
`geometry` crop that removes the holder, and run the eval as above. Check the
registration rows of the table (turn, flip, scale, overlap, correlation) the
first time; an `unaligned` entry in the skipped list means the export does not
look like the render at all (a wrong file, or a crop outside the 60% to 110%
range).

## Licensing

Corpus data is Apache-2.0 (see `LICENSE`). A contributor may instead dedicate a
frame to the public domain under CC0-1.0. Either way, the `licence` field in
the frame's manifest entry must say which, and by setting it the contributor
affirms that they own the frame and have the right to license it. Reference
renders of a frame carry the same licence as the frame.
