# 0023: slider mapping, edits, history, sidecar writes and pool refresh

## Status

Proposed, 2026-10-24 (Rulings 78, 79, 80, 81 and 89).

## Context

The app needs physical sliders, undo, and persistence in the same sidecars the CLI writes. The plan never defines a "Density" control, history is not specified, and the phase 2 sidecar fix (never overwrite a sidecar that could not be read, or that changed on disk) must carry into an app that writes continuously. Roll mode (ADR 0017) means some edits invalidate the pool and others do not.

## Decision

**Sliders (Ruling 78).** The physical sliders map to existing fields; only geometry is new (ADR 0021). The ranges are UI limits (Controller choice), not model defaults, and `Settings::validate` is unchanged except for geometry. A value outside a UI range arriving from a sidecar is shown clamped on the slider but is not rewritten until the user moves it.

| Control | Field | Range | Step | Default |
| --- | --- | --- | --- | --- |
| Density (+ darker) | `print.exposure` | -1.00 to 1.00 log-D | 0.01 | 0 |
| Grade | `print.grade` | 50 to 180 | 1 | 115 |
| Cyan, Magenta, Yellow (+ more of that dye) | `print.cmy[0]`, `[1]`, `[2]` | -0.30 to 0.30 | 0.005 | 0 |
| Toe | `print.toe` (a_sh) | 1.0 to 20.0 | 0.1 | 6.0 |
| Shoulder | `print.shoulder` (a_hl) | 0.5 to 10.0 | 0.1 | 3.0 |
| Black point compensation | `print.black_point_compensation` | toggle | | off |
| Rotate left / right | `geometry.rotate_degrees` | 0, 90, 180, 270 | 90 | 0 |
| Flip | `geometry.flip_horizontal` | toggle | | off |
| Crop | `geometry.crop` | drag on the source-orientation view; width and height at least 0.01; clear = none | | none |
| Analysis buffer | `geometry.analysis_buffer` | 0 to 0.20 | 0.01 | 0.05 |

The roll panel adds the auto colour toggle, the output space select, the `pool_film_base`, `pool_gamma` and `pool_colour` toggles, and the rig and film profile pickers (stored as absolute paths, as the CLI flags do). "Density" is read as print exposure (darker when raised); the plan never defines it, and the alternative readings (film base offset, Dmin) would need new fields.

**Edits and history (Ruling 79).** The single-frame screen edits that frame's `FrameOverrides`; the roll settings panel edits `RollSettings` (its `shared` settings, pool flags and profiles). The frontend sends whole values, never deltas. A drag sends `preview_frame` (no history, no sidecar); release, keyboard entry or a toggle sends `commit_frame` or `commit_roll`. History is one linear in-memory list of `Edit` values (before and after as `Arc`, plus the after hash: `Settings::content_hash` of the resolved frame settings, or `RollSettings::content_hash`), capped at 1000 (oldest dropped). A commit whose value equals the current one is not recorded, and a commit after an undo drops the redo tail. Undo and redo are commits for sidecar purposes. History is not persisted across sessions.

**Sidecar writes (Ruling 80).** Commits mark state dirty. `Session::flush` writes every dirty frame sidecar (`negg_io::write_sidecar`, atomic) and, when the roll settings or pool differ from `roll.negg.json` on disk or it is absent, the roll sidecar (`write_roll_sidecar`, same fields as `negg batch` writes). The Tauri layer calls `flush` 500 ms after the last commit, before any export and on window close; the FFI flushes inside every setter. A frame sidecar holds the resolved settings, the session's estimates, the overrides and `roll = "roll.negg.json"`. Data safety, carrying the phase 2 sidecar fix:

- A frame whose existing sidecar fails to read is `read_only`: commits are refused with `SessionError::ReadOnly` and it is never written.
- Before writing, a sidecar whose modification time differs from the one the session last read or wrote is not overwritten; the frame turns `read_only` and the error is reported.
- Opening a roll writes nothing.

**Pool refresh (Ruling 81).** The session follows the stale-pool rule (Ruling 63) by recomputing, never reusing:

- A change of `shared.profiles` reruns pass 1 for every frame (the rig changes analysis).
- A change of a frame's resolved `geometry.crop` or `geometry.analysis_buffer` rebuilds that frame's `FrameAnalysis` from its cached whole-frame grid (`analyse_grid`, no decode) and reruns `pool_roll`.
- A change of a pool flag, or of any frame's resolved `film.*`, `colour.auto_colour` or `colour.cast`, reruns `pool_roll` over the cached analyses.
- Every other edit (rotate and flip included) leaves the pool.

Plans of affected frames are marked stale and recomputed on demand. Edits are refused (`SessionError::Busy`) until pass 1 has finished for the roll.

**Lock (Ruling 89).** "Lock" in the frame screen is `negg lock` (the same function, moved to `negg-session`) applied as one commit.

## Consequences

- **Slider ranges are not validation.** A sidecar edited by hand may carry values outside the sliders; the engine still accepts anything `validate` accepts.
- **One undo list.** Cost of choosing wrongly: per-frame stacks instead of one list. Memory is bounded by the cap.
- **Debounce.** Cost of a wrong value: a tighter or looser 500 ms.
- **Re-pool cost.** A crop or buffer edit on one frame in a roll triggers `pool_roll` over cached analyses, seconds at worst, and never a decode.
- **Busy during pass 1.** The UI cannot edit before the roll is analysed; the cost is a short wait on open.
- **Concurrent writers.** If the CLI rewrites a sidecar while the app is open, the app notices through the modification time and stops writing that frame instead of clobbering it.

## Eval delta

None, no render change. The sliders map to fields that already exist, with unchanged defaults.
