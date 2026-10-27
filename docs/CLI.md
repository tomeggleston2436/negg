# Command line

The `negg` binary does everything the app does, plus evaluation and profiling.
It is built from `crates/negg-cli` (`cargo build --release -p negg-cli`).

## Commands

```sh
negg convert frame.dng                      # frame.negg.tif (16-bit, sRGB, ICC embedded) and frame.negg.json beside it
negg convert frame.dng --out positives --format jpeg --space adobe_rgb
negg convert frame.dng --settings my.json --dump-stats --timing
negg convert frame.dng --crop 0.04,0.06,0.92,0.88            # crop the export; the analysis sees the crop too
negg convert frame.dng --analysis-area 0.1,0.1,0.8,0.8       # analyse the film only; the export is not cropped
negg batch roll/ --out positives            # every RAW or TIFF in roll/ as one roll: pooled film base, gamma and cast
negg batch roll/ --no-roll                  # each frame on its own, no roll.negg.json
negg lock frame.dng --dmin --gamma          # keep this frame's estimates in later batches
negg batch roll/ --preset Rich              # the Rich tone preset for the roll; frames with their own look keep it
negg preset list                            # the built-in and user tone presets
negg convert frame.dng --preset Soft --web  # a tone preset, plus web/frame.negg.jpg (2048 px, JPEG 85, sRGB)
negg profile rig --flat flat.dng --name rig1 --source "..." --out rig1.rig.negg.toml
negg profile film roll/ --patches patches.csv --stock Portra --iso 160 --name p160 --source "..." --out p160.film.negg.toml
negg eval --corpus corpus --baseline eval-baseline.json --tolerances eval.toml
```

`--crop x,y,w,h` and `--analysis-area x,y,w,h` take four fractions of the frame
in its source orientation. A scan with the film holder in shot pins Dmax at the
density cap and renders dark; an analysis area drawn on the film keeps the
holder out of the estimates without cropping the photo (ADR 0021). A batch
warns for each frame it finds pinned and leaves it out of the gamma and cast
pools (ADR 0032). In roll mode both are stored in `roll.negg.json` and shared by
every frame. `negg batch` also takes `--rig` and `--film` profile files, as
does `convert`.

`--dump-stats` prints the estimates used for the frame (Dmin and its source,
Dmax, gamma and its source, the exposure anchor and `x0`, the cast fit) plus
the grid and image sizes; `--timing` prints per-stage milliseconds to stderr.
The settings file is JSON in the `Settings` schema of `negg-types`; omitted
fields take their defaults, for example
`{"print": {"grade": 90}, "colour": {"auto_colour": false}}`. A sidecar
(recognised by its top-level `settings` key) is also accepted, so a frame's
`<stem>.negg.json` re-renders another frame with the same settings; a JSON
object with neither is refused with exit status 2. The sidecar
`<stem>.negg.json` records the settings, their hash and the estimates, so a
render is reproducible from the source plus that file. Outputs are named
`<stem>.negg.tif` or `<stem>.negg.jpg`, so a TIFF source is never
overwritten, and `negg batch` skips any file whose name contains `.negg.`. A RAW's print
channels are read as Rec.709 primaries and enter Rec.2020 through the BT.2087
matrix (ADR 0026), not the camera matrix; a TIFF with a matrix/TRC ICC
profile is linearised through its TRC in its own primaries and brought into
Rec.2020 by its colorants after the print stage (LUT profiles are refused);
an untagged TIFF is assumed gamma 2.2 and left in its
own colour space. Tone presets (ADR 0034) set only the print grade, toe, shoulder, black point
compensation and saturation, plus a label recorded in `print.preset`. The
built-ins are Standard, Soft, Hard, Flat, Rich and Linear (provisional values).
`--preset <NAME|FILE>` takes a built-in or user preset name or a
`*.preset.negg.toml` file and applies after `--settings`. User presets are
those files in `$NEGG_PRESETS_DIR` when set, else the `negg/presets` folder of
the platform config directory; each is flat TOML with `schema_version = 1`, `name`, `grade`, `toe`,
`shoulder`, `black_point_compensation` and `saturation`.
In `negg batch` roll mode, frames that have their own look keep it;
`--preset-all-frames` replaces them too. `--web` also writes a web copy and
`--web-only` writes only that: 2048 px on the long edge (never upscaled), 8-bit
JPEG at quality 85, sRGB with the ICC profile embedded whatever the output
space, at `web/<stem>.negg.jpg` beside the originals or under `--out`
(ADR 0035). Saturation (`print.saturation`, 0 to 2, default 1; ADR 0033) is
neutral at its default, so renders without a preset or a saturation change are
byte-identical to earlier phases. Settings files are schema 5, frame sidecars
5 and roll sidecars 4; older files migrate on read and newer ones are refused.
Exit status: 0 ok, 1 conversion failure (for `batch`, any
frame), 2 usage or settings error, 3 eval regression.

