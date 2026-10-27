# 0034: tone presets

## Status

Accepted, 2026-10-26 (phase 4, task 2). The built-in values are provisional.

## Context

Phase 4 adds tone presets: named looks that set the print stage's grade, toe, shoulder and black point compensation (step 5 of `docs/MATHS.md`) and the saturation of ADR 0033 (step 7). A preset must never change exposure, CMY, the film estimates, geometry, profiles or the output space, so applying one cannot disturb a frame's corrections.

## Decision

- Add `PrintLook` (grade, toe, shoulder, black point compensation, saturation) and `Preset` (a name and a look) to `negg-types`. A look applies to a `PrintSettings` or to a frame's `PrintOverrides`, and sets the `preset` label with it. `PrintLook::validate` applies the look to a default `Settings` and reuses `Settings::validate`.
- `PrintOverrides::has_own_look` and `clear_look` cover the five look fields and the label only, so clearing a frame's look keeps its exposure and CMY.
- Preset names are trimmed, 1 to 40 characters, with no `/`, `\`, leading `.` or control characters, so a name is safe as a file stem.
- Built-in presets live only in `builtin_presets()`, in this order. Chosen by eye on the corpus, provisional until reviewed in task 10:

| Preset | grade | toe | shoulder | BPC | saturation |
| --- | --- | --- | --- | --- | --- |
| Standard | 115 | 6.0 | 3.0 | off | 1.0 |
| Soft | 95 | 4.0 | 2.0 | off | 0.95 |
| Hard | 135 | 8.0 | 4.0 | off | 1.05 |
| Flat | 85 | 3.0 | 1.5 | off | 0.9 |
| Rich | 125 | 8.0 | 3.0 | on | 1.1 |
| Linear | 115 | 12.0 | 6.0 | off | 1.0 |

- Standard equals the default settings, so choosing it is a no-op on the render.
- User presets are TOML files named `<slug>.preset.negg.toml` (`negg_io::PRESET_SUFFIX`), where the slug is the name in lower case with each space turned into `-` and every character that is not a letter or digit dropped ("My Look!" is `my-look.preset.negg.toml`); saving reuses the file of an existing user preset of the same name, and refuses when the slug's file already exists and holds another preset or cannot be read. A file holds `schema_version = 1`, `name` and the five look keys. They are written atomically like profiles, validated both ways, and a file with a newer schema is refused with an "upgrade negg" message.

## Consequences

- Presets add no render path: they only set existing settings, so a frame with no preset renders as before.
- The `preset` label is not checked against the preset list. Editing a look value by hand keeps the label, and the app shows it as "<name> (modified)".
- Applying a roll preset never edits a read-only frame, as with every roll edit, so such a frame takes the new roll look on its next export unless it has its own full look.
- Cost: the built-in numbers may change after review; changing them affects only frames that pick a preset afterwards.

## Eval delta

None, no render change: Standard is the default look and the corpus is rendered with no preset.
