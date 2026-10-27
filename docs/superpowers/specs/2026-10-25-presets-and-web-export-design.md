# Tone presets, saturation and web export: design

Date: 2026-10-25. Status: draft for the owner's approval. First phase 4 work item.

## Intent

The owner is moving off Negative Lab Pro (NLP) and Lightroom and wants negg to cover the whole job without either. Their NLP workflow was: convert, cycle through NLP's tone profiles to see which suits each photo, sometimes tweak a few sliders, then export, either full size or with an Instagram export that mainly made the picture and the file smaller. The film-stock looks they remember ("Portra", "Gold") were third-party Lightroom presets, not part of NLP, and are not wanted.

So this item adds three things:

1. **Tone presets:** named, one-click sets of negg's existing look sliders, like NLP's tone profiles, with a quick way to compare them on the current photo and to save your own.
2. **Saturation:** one new slider. negg has none today.
3. **Export presets:** "Original" (what export does today) and "Web/Instagram" (same picture and framing, smaller in pixels and bytes, ready to post).

Success: the owner converts a roll, picks a preset per frame (or per roll) by eye in seconds, and exports originals plus web copies in one go, with app and CLI output still byte-identical.

## Scope changes to phase 4 of IMPLEMENTATION_PLAN.md (owner's decisions, 2026-10-25)

- **Linear DNG export: dropped.** It only serves people who keep editing in Lightroom or Capture One; the owner is leaving Lightroom. The plan's phase 4 line and MATHS step 8's "Linear DNG (phase 4)" lines should be updated to say so.
- **Looks as data (3D LUTs, scanner-style colour like NLP's Frontier and Noritsu models): parked.** The owner wants to revisit it together later. Honest scanner looks need either lab scans of negatives also scanned at home (measured) or by-eye looks with descriptive names; that choice is theirs. Do not build any LUT stage in this item.
- The other phase 4 items (wgpu preview, dust and scratch, slide mode, PyO3) are untouched.

## Goals

- A small built-in set of tone presets covering NLP's three tone families:
  - **Standard** (today's defaults), **Soft**, **Hard** (like NLP's LAB family);
  - **Flat**: low contrast with plenty of highlight room (like Cinematic Log and Flat);
  - **Rich**: deeper blacks and stronger mid-tones (like Cinematic Rich);
  - **Linear**: a plain, neutral curve (like NLP's Linear).
- A preset sets only the **look** fields: grade, toe, shoulder, black point compensation and saturation. It never changes density (`exposure`), the CMY offsets, the cast, film estimates, geometry or profiles: those are per-photo corrections, and flicking between presets must not undo them.
- In the frame screen, a strip of small previews of the current photo rendered with each preset; clicking one applies it. A key and a button (like NLP's circular arrow) cycle to the next preset.
- Applying a preset is one undo step. After applying, any slider can be nudged as usual.
- Presets apply at roll scope (the roll's shared settings) or frame scope (a frame override), through the existing roll and override mechanism.
- **Save as preset**: store the current look fields under a name. User presets appear next to the built-in ones. The CLI accepts a preset too (`--preset <built-in name or path>`), so CLI and app output stay byte-identical.
- A **saturation** slider, centred on "unchanged", applied after the step 5 print curve.
- The export dialog offers **Original** and **Web/Instagram**, and both can be ticked to export both in one run, for a frame, a selection or the whole roll.
- **Web/Instagram**: 8-bit JPEG, sRGB with the ICC profile embedded, long edge 2048 pixels (never upscaled), downscaled in the engine in linear light, written so it can never overwrite an original. The CLI gets the same output (`--web`).

## Non-goals

- Film-stock looks (Portra, Gold and so on) and scanner colour models or LUTs (parked, above).
- Presets that change white balance, CMY or the cast.
- Cropping to Instagram shapes (4:5, 1:1), borders, watermarks, output sharpening, metadata or caption handling.
- Linear DNG.
- Arbitrary user-chosen export sizes in the app UI (the CLI may take a numeric long edge if it falls out naturally; see "Left to the builder").

## Anti-goals (failure even if it works)

- Applying or cycling presets loses a frame's density or colour correction.
- App and CLI exports differ for the same settings (Gate 3 invariant).
- Existing sidecars, settings files or golden outputs render differently after the upgrade. A default saturation and no preset must be byte-identical to today; the golden tests must pass unedited apart from schema literals.
- A web copy overwrites or is confused with an original.
- Preset previews make the frame screen sluggish.
- Calling a preset after a real product or film stock ("Frontier", "Portra"). Names describe the look.

## Constraints

- Ground rules of IMPLEMENTATION_PLAN.md and the "Working conventions" section: per-pixel maths lives in `negg-render` only, with a doc comment pointing at the `docs/MATHS.md` step; no default constant changes silently (ADR plus eval delta); `negg-cli eval` runs after engine changes.
- Quantisation happens only at step 8 (ground rule 4). The web downscale runs on the `f32` linear working-space image, after geometry and before the output transform.
- Settings schema bump 4 to 5 for the new saturation field (and the preset label, if stored in settings), with migration and migration tests, frame sidecar and roll sidecar versions bumped accordingly, as done for Ruling 134. A file newer than the build is still rejected (Ruling 144).
- The existing `--settings` roll-mode rule (Ruling 143) still holds; presets are look fields, so they are part of the "look" a `--settings` file replaces.
- Built-in preset values are constants: they live in one place, each with its source noted in an ADR (they are chosen by eye on the corpus, which is an acceptable source if said so, as `TODO(measure)` constants are elsewhere).
- Licensing and dependencies as today; no new image crates unless `cargo deny` passes and the plan's licence list allows them. The owner's private scans stay out of the repo (local tests only).
- Never write dashes as sentence punctuation in code, docs or strings (CI checks).

## Decided (owner)

- DNG dropped; scanner looks parked.
- Three pieces: tone presets, saturation, export presets.
- Presets touch only grade, toe, shoulder, black point compensation and saturation.
- Six built-in presets as listed, modelled on NLP's three families.
- Preview strip plus cycle key and button in the frame screen; one undo step per apply; roll default with frame override; save your own; CLI `--preset`.
- Export: Original plus Web/Instagram, both selectable together; web is size and file size only, no reframing or borders.
- **Roll preset vs frames with their own look (owner, 2026-10-26).** When a preset is applied to the whole roll and some frames already have their own look (any look-field override: grade, toe, shoulder, black point compensation, saturation, or a frame-scope preset), the app asks which to do. Wording is the builder's, along the lines of: "3 frames have their own look. Apply Rich to: All frames / Only frames without their own look". "All frames" clears those frames' look-field overrides (and only those fields: density, CMY, geometry and other overrides stay); the other choice leaves them alone. With no such frames, no question is asked. The whole apply, roll plus cleared frames, is one undo step, which needs a compound history entry (history today records one frame or the roll per entry) and matching session, FFI and IPC operations.

## Controller's calls (owner did not decide; flagged for review)

- **A preset copies values; it is not a live reference.** Applying a preset writes its values into the settings, so a sidecar renders the same even if the preset file later changes or is deleted. The settings also record the preset's name as a label, so the UI can show "Rich" or "Rich (modified)" when a slider has moved since. Whether the label lives in `Settings` (and so in the hash) or beside it in the sidecar is the builder's call; the render must not depend on it.
- **User presets** are small TOML files (`*.preset.negg.toml`, same style as rig and film profiles) in a per-user folder: the platform config directory (for example `~/Library/Application Support/negg/presets` on macOS). The CLI's `--preset` takes a built-in name or a path to such a file. Presets are not tied to a roll.
- **Saturation** is a scale on chroma around a neutral axis, applied in step 7 after the entry matrix (so in linear Rec.2020) and before the lcms output conversion. Step 6 runs before step 5 in log space, so this is after both. Default 1.0 (unchanged), range about 0 to 2; exactly 1.0 skips the step so existing outputs stay byte-identical. For untagged sources (entry matrix `None`), the same formula with the same Rec.2020 weights runs on the working image as it stands, the way the rest of step 7 already treats such sources, so preview and export agree for every source. The exact formula (and whether luminance is preserved by Rec.2020 Y) is documented in MATHS step 7 with an ADR.
- **Where the preset strip applies.** The strip in the frame screen applies at frame scope (a frame override), like every other frame-screen edit (ADR 0023, Ruling 79). The roll settings panel gets its own preset picker that applies at roll scope.
- **CLI roll preset.** `negg batch --preset X` keeps frames' own look overrides (the safe choice, matching how `resolve` already lets frame overrides win); a separate flag (for example `--preset-all-frames`) clears their look-field overrides, matching the app's "All frames". Parity tests cover both.
- **CLI web output.** `--web` writes the web copies in addition to the normal output; `--web-only` writes only the web copies. That gives every app choice (Original, Web, both) a CLI equivalent for the parity tests. Sidecar and roll sidecar writes are the same as for any export today: a web-only export still records its settings.
- **Web/Instagram constants:** long edge 2048 px, JPEG quality 85 (the thumbnail quality; originals stay at 92), sRGB regardless of the frame's `output_space`, never upscaled. One ADR records them.
- **Web output location:** a `web/` subfolder of the export destination (beside the sources when no folder is chosen, as today), same base file name with `.jpg`. Exporting both renders each frame once and writes both files.
- **Downscale filter:** a proper resampling filter in linear light (for example Lanczos-3 or an area-weighted box at non-integer ratios), implemented in `negg-render`, deterministic and documented in MATHS step 8. Not the preview pyramid's 2 x 2 box, which only does halves.
- This work does not block Gate 3; the outstanding Gate 3 hand tests are separate.

## Left to the builder

- The built-in preset values (grade, toe, shoulder, BPC, saturation per preset), tuned by eye on the corpus, within the existing valid ranges, recorded in an ADR. The owner reviews them by eye before they are final.
- Whether "Linear" needs paper Dmin and Dmax changes; if so, either add those to the preset field list (with an ADR) or approximate with the five allowed fields.
- How preset previews are rendered (likely the cached analysis grid render at small size, as thumbnails are) and cached, and when they refresh.
- The cycle key binding (must not clash with existing shortcuts or text-field input) and the strip's placement in the frame screen.
- How "Save as preset" names and overwrites, and how a user deletes a preset (deleting from the folder is acceptable for a first version).
- Session, FFI and IPC surface for listing presets, applying one at roll or frame scope, saving one, and the export request's new options; the C ABI follows the existing patterns.
- CLI flag spelling beyond `--preset` and `--web` (for example whether `--long-edge N` is exposed).
- Task breakdown and ordering. A natural split: (1) saturation in the engine plus schema 5; (2) presets in types, io and CLI; (3) web downscale and export in render, io, session and CLI; (4) app UI for presets and saturation; (5) export dialog; (6) docs and plan updates.

## Verification

- Golden and parity tests pass with no preset and default saturation, unedited apart from schema literals.
- New parity tests: the same frame with a preset and with saturation, exported by the app session and by `negg convert` or `negg batch --preset`, byte-identical; the same for `--web`.
- Migration tests from schema 4 sidecars, settings files and roll sidecars.
- A test that applying and cycling presets leaves exposure, CMY, cast, geometry and profiles unchanged.
- A web export test: long edge 2048 (and an image smaller than 2048 unchanged in size), sRGB ICC embedded, written under `web/`, never touching the original file.
- `negg-cli eval` runs with no metric change at defaults.
