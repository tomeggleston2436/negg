# 0023: slider mapping, edits, history, sidecar writes and pool refresh

## Status

Accepted, 2026-10-24 (Rulings 78, 79, 80, 81 and 89; implemented in Wave 2 and amended by Rulings 122 to 126 and 134 to 137, below).

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
| Analysis area (Ruling 134) | `geometry.analysis_area` | drag on the source-orientation view with the analysis area tool, or numeric entry; width and height at least 0.01; clear = none | | none |

The roll panel adds the auto colour toggle, the output space select, the `pool_film_base`, `pool_gamma` and `pool_colour` toggles, and the rig and film profile pickers (stored as absolute paths, as the CLI flags do). "Density" is read as print exposure (darker when raised); the plan never defines it, and the alternative readings (film base offset, Dmin) would need new fields.

**Edits and history (Ruling 79).** The single-frame screen edits that frame's `FrameOverrides`; the roll settings panel edits `RollSettings` (its `shared` settings, pool flags and profiles). The frontend sends whole values, never deltas. A drag sends `preview_frame` (no history, no sidecar); release, keyboard entry or a toggle sends `commit_frame` or `commit_roll`. History is one linear in-memory list of `Edit` values (before and after as `Arc`, plus the after hash: `Settings::content_hash` of the resolved frame settings, or `RollSettings::content_hash`), capped at 1000 (oldest dropped). A commit whose value equals the current one is not recorded, and a commit after an undo drops the redo tail. Undo and redo are commits for sidecar purposes. History is not persisted across sessions.

**Sidecar writes (Ruling 80).** Commits mark state dirty. `Session::flush` writes every dirty frame sidecar (`negg_io::write_sidecar`, atomic) and, when the roll settings or pool differ from `roll.negg.json` on disk or it is absent, the roll sidecar (`write_roll_sidecar`, same fields as `negg batch` writes). The Tauri layer calls `flush` 500 ms after the last commit, before any export and on window close; the FFI flushes inside every setter. A frame sidecar holds the resolved settings, the session's estimates, the overrides and `roll = "roll.negg.json"`. A flush first settles any pending re-pool (below), so the estimates it writes are the ones `negg batch` would write. A flush before pass 1 has finished writes the sidecar without estimates, as for an unanalysed frame, and the first flush after pass 1 and its pool writes them. Data safety, carrying the phase 2 sidecar fix:

- A frame whose existing sidecar fails to read is `read_only`: commits are refused with `SessionError::ReadOnly` and it is never written.
- Before writing, a sidecar that changed on disk since the session last read or wrote it is not overwritten; the frame turns `read_only`, its overrides are read again from disk, its edits leave the history, and `SessionError::Stale` is reported. By Ruling 122 a sidecar counts as changed only when its modification time or length differs and its bytes differ too, so a program that wrote the very same bytes (as `negg batch` does after an export) has not changed it.
- The roll sidecar is checked the same way. If `roll.negg.json` changed on disk it is not overwritten and the flush and export report `Stale` until the roll is opened again (Ruling 123; the app shows the error).
- Opening a roll writes nothing. The roll's settings are written only when something is committed (a roll edit, including `set_roll_analysis_area` before pass 1 (ADR 0021), or any frame edit, whose sidecar names `roll.negg.json`), or when an existing `roll.negg.json` is out of date after a completed analysis. A session that only browsed (no edit, no area set, whether or not it analysed) writes no `roll.negg.json` on close or flush, and so no frame sidecar either (Ruling 141): the next open of a folder never batched waits for Analyse again (Ruling 136), the intended first-run behaviour.
- **Unedited frames are brought up to date (Ruling 139).** A roll edit (an analysis area, a crop, a pool flag, a profile) or another frame's edit can change a frame's resolved settings or its estimates without that frame being edited. Once the pool is final, a flush therefore also rewrites every writable frame that pass 1 analysed whose sidecar differs from the record it would now write (the negg version that wrote it aside), with the same changed-on-disk guard and never for a read-only or stale frame, so every sidecar is what `negg batch` would write and `negg lock` locks current values. This runs only when the session writes the roll's sidecars at all (the roll has a `roll.negg.json`, or an edit was made), and not while `roll.negg.json` changed behind the session: then only the frames edited in the session are written, so their edits are not lost, and the other frames wait until the roll is opened again.
- **An edit of a changed sidecar is refused up front.** Every frame edit (commit, preview, lock) first checks the frame's sidecar (a stat, and a read when the stamp moved): a sidecar changed behind the session turns the frame read-only and the edit is refused with `SessionError::Stale`, instead of being accepted and then dropped by the next flush. The changed-on-disk check at flush stays for a change that lands between an edit and the flush.
- **What the session saw.** The bytes and the stamp the session compares are taken from one open file when it reads a sidecar (and parsed from those very bytes), and from the open file before the rename when it writes one (`negg_io::write_file_atomic`), never read back from the path afterwards, so another writer cannot slip in between.
- Every atomic write in `negg-io` (sidecars, roll sidecars, profiles, JPEG and TIFF outputs) goes through one temporary file of a unique name in the same directory (the file name, the process id and a counter), so the app and the CLI writing the same file never share a temporary file.
- The app flushes before it opens another roll, on window close and on quit. If that flush fails the action is refused once with the error kind `unsaved` (ADR 0020, Ruling 131) and the next attempt goes ahead without the edits.

**Pool refresh (Ruling 81).** The session follows the stale-pool rule (Ruling 63) by recomputing, never reusing:

- A change of `shared.profiles` reruns pass 1 for every frame (the rig changes analysis).
- A change of a frame's resolved analysis (`Geometry::same_analysis`: the analysis area, else the crop, and when there is one the analysis buffer; Ruling 134) rebuilds that frame's `FrameAnalysis` from its cached whole-frame grid (`analyse_grid`, no decode) and reruns `pool_roll`. A change of the roll's analysis area does so for every frame that follows it.
- A change of a pool flag, or of any frame's resolved `film.*`, `colour.auto_colour` or `colour.cast`, reruns `pool_roll` over the cached analyses.
- Every other edit (rotate and flip included, and a crop that an analysis area overrides) leaves the pool.

**Deferred re-pooling (Rulings 124 and 126).** Re-pooling a 36-frame roll took seconds, which froze the app on every edit, so a commit no longer waits for it. A commit, an undo, a redo or a lock that changes what the pool sees marks the pool pending and returns at once with the frame planned on the old pool. The host drives the re-pool through `Session::analyse_next`, exactly as it drives pass 1: the call makes the pending re-pool and returns `AnalyseStep::Pooled`, then `Done`. `pool_pending` (serde default false) on `RollView`, `FrameView` and `HistoryView` tells the host, in every edit's own return value, that it must call `analyse_next` until Done and then fetch its views again (the generation has moved when the pool changed). `flush`, `lock_frame` and `export` run a pending re-pool first. Plan and thumbnail caches are keyed on the frame's effective pooled values and settings hash, not on a pool epoch, so a re-pool that changes nothing keeps every cache. In the app the analyse loop also runs pending re-pools and the screens show a "re-pooling" indicator and fetch their views again when it clears (Ruling 127). While a re-pool is pending the estimates and the `needs_analysis_area` flag shown are those of the last completed pool and analysis.

Plans of affected frames are marked stale and recomputed on demand. Edits are refused (`SessionError::Busy`) until pass 1 has finished for the roll; a re-pool does not block them. The one edit allowed before pass 1 is `Session::set_roll_analysis_area` (ADR 0021), which the app uses in its pre-analysis step.

**Lock (Ruling 89).** "Lock" in the frame screen is `negg lock` (the same function, moved to `negg-session`) applied as one commit.

## Consequences

- **Slider ranges are not validation.** A sidecar edited by hand may carry values outside the sliders; the engine still accepts anything `validate` accepts.
- **One undo list.** Cost of choosing wrongly: per-frame stacks instead of one list. Memory is bounded by the cap.
- **Debounce.** Cost of a wrong value: a tighter or looser 500 ms.
- **Re-pool cost.** A crop, analysis area or buffer edit on one frame in a roll triggers `pool_roll` over cached analyses, seconds at worst, and never a decode. Since Ruling 124 the cost is a visible pending flag rather than a frozen edit: `pool_roll` runs the per-frame cast fits in parallel (rayon, results in input order, the first error in input order as before), the session rebuilds stale frame analyses on several threads, and `cast.rs` takes percentiles by selection instead of a full sort, with the output bitwise unchanged. Cost of choosing wrongly: the estimates on screen lag the edit by the re-pool.
- **Busy during pass 1.** The UI cannot edit frames or the other roll settings before the roll is analysed; the cost is a short wait on open. In the app the wait starts when the user presses Analyse for a roll opened for the first time (ADR 0021).
- **Concurrent writers.** If the CLI rewrites a sidecar while the app is open, the app notices (a changed modification time or length and changed bytes, Ruling 122) and stops writing that frame instead of clobbering it. A roll sidecar that changed blocks the flush and the export until the roll is opened again (Ruling 123).

## Eval delta

None, no render change. The sliders map to fields that already exist, with unchanged defaults.
