# Tone presets, saturation and web export Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add NLP-style tone presets (built-in and user-saved), a saturation slider, and an Original plus Web/Instagram export, in engine, CLI, session, FFI and app, with app and CLI output still byte-identical.

**Architecture:** Saturation and a preset label become two new `PrintSettings` fields (schema 5), applied in the fused step 7 kernel through `PrintCoefficients`, so no render signature changes. A preset is a `PrintLook` (five look fields) whose values are copied into settings or overrides; the session adds frame and roll preset operations, the roll one recorded as a single compound history entry. Web export is a `WebOutput` option on the existing flows: after geometry, an area-average downscale in linear light to a 2048 px long edge, then sRGB JPEG at quality 85 into a `web/` subfolder.

**Tech Stack:** Rust workspace (`negg-types`, `negg-io`, `negg-render`, `negg-session`, `negg-cli`, `negg-ffi`), `toml`, `serde`, `dirs` 6 (new, MIT OR Apache-2.0); Tauri 2 app with React, TypeScript and Vite (`app/`), vitest.

**Spec:** `docs/superpowers/specs/2026-10-25-presets-and-web-export-design.md` (approved by the owner 2026-10-26). Read it before any task.

## Global Constraints

- **Never run `git commit`, `git add`, `git push` or any history rewrite** (owner's global rule). Every task ends with its changes left in the working tree; the "Leave for review" step replaces the usual commit step.
- No em dash (U+2014) or en dash (U+2013) anywhere in code, comments, docs, strings or messages. CI greps `*.rs *.toml *.md *.yml *.yaml *.json *.ts *.tsx`. Rewrite the sentence instead.
- British English in docs and messages. Every new `.rs` file under `crates/` starts with `// SPDX-License-Identifier: MPL-2.0` (engine crates: types, io, render) or `// SPDX-License-Identifier: Apache-2.0` (session, cli, ffi). New app files start with `// SPDX-License-Identifier: Apache-2.0`.
- Per-pixel maths lives only in `negg-render`, each public function doc-commented with its `docs/MATHS.md` step. Quantisation only at step 8.
- No preset and default saturation must render byte-identically to today: `crates/negg-cli/tests/data/golden.phase1.negg.tif` and the parity tests pass unedited apart from schema-version literals.
- Schema: `SETTINGS_SCHEMA_VERSION` 4 -> 5, `SIDECAR_SCHEMA_VERSION` 4 -> 5, `ROLL_SIDECAR_SCHEMA_VERSION` 3 -> 4. Files newer than the build are still rejected (Ruling 144).
- Presets touch only grade, toe, shoulder, black point compensation and saturation (plus the label). Never exposure, CMY, cast, film estimates, geometry, profiles or output space.
- Web/Instagram: long edge 2048 px, never upscaled; 8-bit JPEG quality 85; sRGB with ICC embedded whatever the frame's `output_space`; written to `<export dir>/web/<stem>.negg.jpg`; never overwrites an original.
- No 3D LUT stage, no DNG, no reframing, borders, watermarks or output sharpening.
- Checks after every Rust task: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace --all-features`, `cargo deny check`. After engine tasks also `cargo run --release -p negg-cli -- eval --corpus corpus --out eval-results.json --baseline eval-baseline.json --tolerances eval.toml` (no metric change at defaults). App tasks: in `app/`, `pnpm exec tsc --noEmit`, `pnpm exec vitest run`, `pnpm build`, `node scripts/check-licences.mjs`; in `app/src-tauri/`, `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.
- Dash check before reporting: `git grep -nP '[\x{2014}\x{2013}]' -- '*.rs' '*.toml' '*.md' '*.ts' '*.tsx' ':!IMPLEMENTATION_PLAN.md' ':!LICENSE*'` plus the same `grep -rnP` over new untracked files must find nothing.
- The owner's private scans stay out of the repo; any corpus renders for review go to the scratchpad.

## Review Focus

1. **A roll preset on a roll whose frames have mixed overrides** (some only a grade nudge, some a frame preset, some exposure or CMY only): "All frames" must clear only look fields, never exposure, CMY or geometry; "without own look" must leave any frame with any look override untouched. Test: Task 4 `roll_preset_scopes_touch_only_look_fields`.
2. **Undo and redo of a roll preset that cleared frames**, including after one of those frames turned read-only: one undo restores roll and every frame; a read-only frame's part is dropped, the rest still undoes. Test: Task 4 `roll_preset_is_one_undo_step` and `roll_preset_undo_skips_a_frame_that_turned_read_only`.
3. **Web export of a frame already smaller than 2048 px on the long edge, or with a crop that makes it so:** written at its own size, not upscaled, still sRGB quality 85. Test: Task 6 `web_never_upscales` and Task 7 `web_export_of_a_small_crop_keeps_its_size`.
4. **A user preset file that is malformed, has an out-of-range value, or reuses a built-in name:** listing skips it with a warning instead of failing; `--preset` naming it fails with a message naming the file. Test: Task 3 `list_skips_bad_user_presets_with_a_warning`.
5. **An old (schema 4) sidecar or roll sidecar opened by the new build then flushed:** loads with saturation 1 and no label, and renders and writes byte-identical images. Test: Task 1 `schema_4_files_migrate_to_5_unchanged` and Task 7 parity tests.

---

### Task 1: Saturation field, preset label and schema 5

**Files:**
- Modify: `crates/negg-types/src/settings.rs` (PrintSettings, validate, print_floats, schema const and doc)
- Modify: `crates/negg-types/src/roll.rs` (PrintOverrides, resolve)
- Modify: `crates/negg-render/src/print.rs` (PrintCoefficients carries saturation)
- Modify: `crates/negg-render/src/full.rs` (apply saturation in `render_in_place` after the matrix)
- Create: `crates/negg-render/src/saturation.rs` (kernel), export from `crates/negg-render/src/lib.rs`
- Modify: `crates/negg-io/src/sidecar.rs`, `crates/negg-io/src/roll.rs` (ladders v4->v5 and roll v3->v4)
- Modify: `crates/negg-io/tests/migrations.rs` and add fixtures under `crates/negg-io/tests/data/migrations/`
- Modify: schema literals in existing tests that assert version numbers (search `SCHEMA_VERSION` and literal `4`/`3` in sidecar asserts)
- Modify: `app/src-tauri/tests/fixtures.rs` and regenerate fixtures with `NEGG_BLESS=1`
- Modify: `docs/MATHS.md` step 7 (saturation subsection) and step 8 (schema paragraph)
- Create: `docs/adr/0033-saturation.md`; add to `docs/adr/README.md`

**Interfaces:**
- Produces: `PrintSettings.saturation: f32` (default 1.0, valid 0.0..=2.0, finite), `PrintSettings.preset: Option<String>` (label only; default None). Serde: `saturation` skipped when exactly 1.0, `preset` skipped when None, so a default settings value serialises exactly as schema 4 apart from `schema_version`.
- Produces: `PrintOverrides.saturation: Option<f32>`, `PrintOverrides.preset: Option<String>`, merged by `resolve` like the other print fields.
- Produces: `negg_render::saturate_pixel(rgb: [f32; 3], s: f32) -> [f32; 3]`; `PrintCoefficients::saturation(&self) -> f32`.

- [ ] **Step 1: Write the failing tests**

In `crates/negg-render/src/saturation.rs` tests:
```rust
#[test]
fn saturation_one_is_identity_bitwise() {
    for rgb in [[0.2, 0.5, 0.9], [0.0, 0.0, 0.0], [1.0, 0.3, 0.0]] {
        assert_eq!(saturate_pixel(rgb, 1.0), rgb);
    }
}
#[test]
fn saturation_zero_gives_rec2020_luminance_grey() {
    let rgb = [0.2f32, 0.5, 0.9];
    let y = 0.2627 * rgb[0] + 0.6780 * rgb[1] + 0.0593 * rgb[2];
    for c in saturate_pixel(rgb, 0.0) { assert!((c - y).abs() < 1e-6); }
}
#[test]
fn saturation_preserves_luminance_and_neutrals() {
    let grey = [0.4f32; 3];
    assert_eq!(saturate_pixel(grey, 1.7), grey);
    let rgb = [0.2f32, 0.5, 0.9];
    let out = saturate_pixel(rgb, 1.5);
    let y = |p: [f32; 3]| 0.2627 * p[0] + 0.6780 * p[1] + 0.0593 * p[2];
    assert!((y(out) - y(rgb)).abs() < 1e-6);
    assert!(out.iter().all(|c| *c >= 0.0));
}
```
In `crates/negg-render/src/full.rs` tests: `fused_render_with_saturation_equals_the_composition` (composition = existing composition then `saturate_pixel` per pixel, for `matrix` `None` and `Some`) and `default_saturation_renders_bitwise_as_before` (render with `PrintSettings::default()` equals render with `saturation: 1.0` explicitly; and equals the pre-change composition helper).
In `crates/negg-types/src/settings.rs` tests: `default_settings_json_has_no_saturation_or_preset_key` (serialise default, assert `!json.contains("saturation") && !json.contains("preset")`), `saturation_out_of_range_is_invalid` (`-0.1`, `2.1`, `NaN` each fail `validate` with a message naming `saturation`), `resolve_takes_saturation_and_preset_overrides`.
In `crates/negg-io/tests/migrations.rs`: `schema_4_files_migrate_to_5_unchanged`: a schema 4 sidecar fixture and a schema 3 roll sidecar fixture load with `saturation == 1.0`, `preset == None`, `schema_version == 5` (roll: 4, settings 5), other fields equal to the parsed old values.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p negg-render -p negg-types -p negg-io`
Expected: compile errors for `saturate_pixel`, `saturation`, `preset`.

- [ ] **Step 3: Implement**

- `saturate_pixel(rgb, s)`: `y = 0.2627 r + 0.6780 g + 0.0593 b` (Rec.2020 luminance); `out_c = max(0, y + s * (c - y))`; return `rgb` unchanged when `s == 1.0` (exact compare, so the default path is bitwise identical).
- `PrintCoefficients::new` copies `print.saturation`; `render_in_place` applies `saturate_pixel(_, print.saturation())` to the step 7 result (after the matrix, or after print when `matrix` is `None`), skipping the call when saturation is exactly 1.0.
- Validation: saturation finite and in `0.0..=2.0`; add it to `print_floats` (array length 10).
- Ladder: `upgrade_v4_to_v5` sets `schema_version` and `settings.schema_version` to 5 and nothing else; `upgrade_roll_v3_to_v4` uses `set_roll_versions(value, 4, 5)`. Update the ladder doc comments and the module docs that list versions.
- MATHS step 7: a "Saturation (phase 4, ADR 0033)" paragraph with the formula, its place (after the entry matrix, before the lcms output transform; untagged sources use the same weights on their own RGB), default 1 skips the step. Step 8: schema 5 sentence.
- ADR 0033: decision as above, storage in `PrintSettings` because `PrintCoefficients` already reaches the fused kernel; Eval delta "none at the default".

- [ ] **Step 4: Run tests and all checks**

Run the Global Constraints Rust checks and eval. Then in `app/src-tauri`: `NEGG_BLESS=1 cargo test` once, then `cargo test`.
Expected: all pass; golden unchanged; eval no metric change.

- [ ] **Step 5: Leave for review** (no commit; list changed files in the report)

---

### Task 2: PrintLook, built-in presets and preset files

**Files:**
- Create: `crates/negg-types/src/look.rs`; export from `crates/negg-types/src/lib.rs`
- Create: `crates/negg-io/src/preset.rs`; export from `crates/negg-io/src/lib.rs`
- Create: `docs/adr/0034-tone-presets.md`; add to `docs/adr/README.md`

**Interfaces:**
- Consumes: Task 1 fields.
- Produces (negg-types):
  - `pub struct PrintLook { pub grade: f32, pub toe: f32, pub shoulder: f32, pub black_point_compensation: bool, pub saturation: f32 }` (Clone, Copy, Debug, PartialEq, Serialize, Deserialize)
  - `pub struct Preset { pub name: String, pub look: PrintLook }` (Clone, Debug, PartialEq, Serialize, Deserialize)
  - `pub fn builtin_presets() -> Vec<Preset>`, in this order: Standard, Soft, Hard, Flat, Rich, Linear
  - `impl PrintLook { pub fn of(print: &PrintSettings) -> PrintLook; pub fn apply_to_settings(&self, name: &str, print: &mut PrintSettings); pub fn apply_to_overrides(&self, name: &str, o: &mut PrintOverrides); pub fn validate(&self) -> Result<()> }`
  - `impl PrintOverrides { pub fn has_own_look(&self) -> bool; pub fn clear_look(&mut self) }`: the look fields are grade, toe, shoulder, black_point_compensation, saturation, preset
  - `pub fn validate_preset_name(name: &str) -> Result<()>`: trimmed, 1 to 40 characters, no `/`, `\`, `.` at start, or control characters
- Produces (negg-io): `pub const PRESET_SUFFIX: &str = ".preset.negg.toml"`, `pub fn read_preset(path: &Path) -> Result<Preset, IoError>`, `pub fn write_preset(path: &Path, preset: &Preset) -> Result<(), IoError>` (atomic like profiles, file carries `schema_version = 1`, `name`, and the five look keys).

- [ ] **Step 1: Write the failing tests**

negg-types `look.rs` tests:
- `builtins_are_six_valid_named_looks`: names exactly `["Standard","Soft","Hard","Flat","Rich","Linear"]`, each `look.validate()` ok.
- `standard_is_the_default_look`: `builtin_presets()[0].look == PrintLook::of(&PrintSettings::default())`.
- `apply_to_overrides_sets_only_look_fields`: start from overrides with `exposure: Some(0.2)`, `cmy: Some([0.01, 0.0, -0.02])`, `paper_dmin: Some(0.07)`; apply "Rich"; assert exposure, cmy, paper_dmin unchanged and the five look fields plus `preset == Some("Rich")` set.
- `clear_look_keeps_corrections`: same start plus look fields; after `clear_look`, `has_own_look()` is false and exposure, cmy, paper_dmin are unchanged.
- `preset_names_are_validated`: `""`, `"   "`, `"a/b"`, `".hidden"`, 41 characters fail; `"My warm"` passes.
negg-io `preset.rs` tests: `preset_round_trips_through_toml`; `preset_with_out_of_range_grade_is_rejected` (grade 200, error mentions `grade`); `preset_newer_schema_is_rejected` (`schema_version = 2`, message says upgrade negg).

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p negg-types -p negg-io`
Expected: FAIL to compile (`look` module missing).

- [ ] **Step 3: Implement**

Provisional built-in values (owner reviews by eye in Task 10; record them and "chosen by eye on the corpus, provisional" in ADR 0034):

| Preset | grade | toe | shoulder | BPC | saturation |
| --- | --- | --- | --- | --- | --- |
| Standard | 115 | 6.0 | 3.0 | false | 1.0 |
| Soft | 95 | 4.0 | 2.0 | false | 0.95 |
| Hard | 135 | 8.0 | 4.0 | false | 1.05 |
| Flat | 85 | 3.0 | 1.5 | false | 0.9 |
| Rich | 125 | 8.0 | 3.0 | true | 1.1 |
| Linear | 115 | 12.0 | 6.0 | false | 1.0 |

The values live only in `builtin_presets()`. `PrintLook::validate` reuses `Settings::validate` on a default `Settings` with the look applied.

- [ ] **Step 4: Run tests and all checks** (Global Constraints Rust checks)

- [ ] **Step 5: Leave for review**

---

### Task 3: Preset store (list, find, save) and the user presets folder

**Files:**
- Create: `crates/negg-session/src/presets.rs`; export from `crates/negg-session/src/lib.rs`
- Modify: `crates/negg-session/Cargo.toml` (add `dirs = "6"`), root `Cargo.toml` workspace deps if the workspace pins there

**Interfaces:**
- Consumes: Task 2 types and IO.
- Produces:
  - `pub const PRESETS_DIR_ENV: &str = "NEGG_PRESETS_DIR"`
  - `pub fn presets_dir() -> Option<PathBuf>`: `$NEGG_PRESETS_DIR` when set, else `dirs::config_dir()/negg/presets`
  - `pub struct PresetList { pub presets: Vec<Preset>, pub warnings: Vec<String> }` (Serialize, Deserialize)
  - `pub fn list_presets(dir: Option<&Path>) -> PresetList`: built-ins first in their order, then user presets sorted by name (case-insensitive). A missing folder is no user presets and no warning. A file that fails to read or validate, or whose name equals a built-in name (case-insensitive) or an earlier user preset's, is skipped with one warning naming the file.
  - `pub fn find_preset(name_or_path: &str, dir: Option<&Path>) -> Result<Preset, ConvertError>`: a value ending in `PRESET_SUFFIX` or containing a path separator is read as a file; otherwise a built-in or user preset by name (case-insensitive). Unknown names list the available names in the error.
  - `pub fn save_preset(dir: &Path, name: &str, look: &PrintLook) -> Result<PathBuf, ConvertError>`: validates the name, refuses a built-in name, creates the folder, writes `<slug>.preset.negg.toml` (slug: lower case, spaces to `-`, other non-alphanumerics dropped), overwriting a user preset of the same name.

- [ ] **Step 1: Write the failing tests** (each uses a `tempfile` folder passed as `dir`, never the real config folder)
- `list_without_a_folder_is_the_builtins`
- `list_puts_user_presets_after_builtins_sorted`
- `list_skips_bad_user_presets_with_a_warning`: a malformed TOML, a grade 300 file and a file named "Rich" each skipped, three warnings, each naming its file
- `find_by_name_is_case_insensitive_and_reads_paths`
- `find_unknown_name_lists_the_available_names`
- `save_refuses_builtin_names_and_overwrites_own`

- [ ] **Step 2: Run to verify failure**: `cargo test -p negg-session presets` (compile error)
- [ ] **Step 3: Implement** the signatures above
- [ ] **Step 4: Run tests and all checks**; `cargo deny check` must accept `dirs`
- [ ] **Step 5: Leave for review**

---

### Task 4: Session preset operations, compound undo and preset previews

**Files:**
- Modify: `crates/negg-session/src/session.rs` (new `Edit::Preset` variant, undo, redo, read-only purge near the current `Edit::Frame` purge, new methods)
- Modify: `crates/negg-session/src/view.rs` (`RollPresetScope`)
- Test: `crates/negg-session/src/session/tests.rs`

**Interfaces:**
- Consumes: Task 2 `PrintLook`, `Preset`, `PrintOverrides::{has_own_look, clear_look}`.
- Produces:
  - `#[serde(rename_all = "snake_case")] pub enum RollPresetScope { AllFrames, FramesWithoutOwnLook }` in `view.rs`
  - `Session::apply_frame_preset(&mut self, frame: u32, preset: &Preset) -> Result<FrameView, SessionError>`: exactly `commit_frame` with the frame's overrides after `preset.look.apply_to_overrides(&preset.name, ..)`
  - `Session::frames_with_own_look(&self) -> Vec<u32>`: indices of writable frames whose overrides `has_own_look()`
  - `Session::apply_roll_preset(&mut self, preset: &Preset, scope: RollPresetScope) -> Result<RollView, SessionError>`: sets the roll's shared look with `apply_to_settings`; with `AllFrames` also `clear_look()` on every writable frame with its own look; records one `Edit::Preset { roll_before, roll_after, frames: Vec<(usize, Arc<FrameOverrides>, Arc<FrameOverrides>)>, hash }`; marks changed frames dirty and the roll dirty. `Busy` before pass 1; nothing recorded when nothing changes.
  - `Session::preset_thumbnail_jpeg(&mut self, frame: u32, look: &PrintLook) -> Result<Vec<u8>, SessionError>`: the frame's thumbnail as `thumbnail_jpeg` would render it if its resolved settings had `look` applied (plan re-solved with those print settings). Cached per frame by (thumbnail key, look); the cache is cleared whenever the frame's thumbnail cache is.
- Undo of `Edit::Preset`: apply `roll_before`, then each frame's `before`; redo the reverse order with `after`. When a frame turns read-only, its entry is removed from every `Edit::Preset` (the roll part stays).

- [ ] **Step 1: Write the failing tests**
- `frame_preset_sets_only_look_fields`: frame with exposure and CMY overrides; apply "Hard"; resolved settings have Hard's look and label, the same exposure and CMY.
- `roll_preset_scopes_touch_only_look_fields`: frames 0 (grade nudge only), 1 (frame preset "Soft"), 2 (exposure and CMY only), 3 (nothing). `FramesWithoutOwnLook` with "Rich": frames 0 and 1 keep their own look; 2 and 3 resolve to Rich with their exposure and CMY intact. Repeat from a fresh session with `AllFrames`: all four resolve to Rich; frame 2 keeps exposure and CMY; frame 0 and 1 overrides have no look fields.
- `frames_with_own_look_lists_look_overrides_only`: returns `[0, 1]` for the set-up above.
- `roll_preset_is_one_undo_step`: after `AllFrames`, `history_view().len` grew by 1; one `undo` restores the roll and frames 0 and 1 overrides exactly; `redo` re-applies.
- `roll_preset_undo_skips_a_frame_that_turned_read_only`: make frame 1's sidecar unreadable after the apply, trigger the read-only path as existing tests do, then `undo`: roll and frame 0 restored, no error.
- `preset_thumbnail_equals_thumbnail_after_applying`: `preset_thumbnail_jpeg(f, &rich.look)` bytes equal `thumbnail_jpeg(f)` after `apply_frame_preset(f, &rich)`.

- [ ] **Step 2: Run to verify failure**: `cargo test -p negg-session preset`
- [ ] **Step 3: Implement** the interfaces above
- [ ] **Step 4: Run tests and all checks**
- [ ] **Step 5: Leave for review**

---

### Task 5: CLI `--preset`, `--preset-all-frames` and `negg preset list`

**Files:**
- Modify: `crates/negg-cli/src/cli.rs` (OutputArgs flags; new `Preset` subcommand with `List`)
- Modify: `crates/negg-cli/src/convert.rs`, `crates/negg-cli/src/roll.rs` (map args to `OutputRequest`)
- Modify: `crates/negg-session/src/flow.rs` (`OutputRequest.preset: Option<Preset>`, `OutputRequest.preset_all_frames: bool`)
- Test: `crates/negg-cli/tests/presets.rs` (new), `crates/negg-cli/tests/app_parity.rs`

**Interfaces:**
- Consumes: Task 3 `find_preset`, `list_presets`, `presets_dir`; Task 4 session operations (parity test).
- Produces:
  - `--preset <NAME|FILE>` on convert and batch: resolved with `find_preset(_, presets_dir())`. Applied after `--settings`: without roll mode, to the frame's settings with `apply_to_settings`; in roll mode, to the roll's shared settings, which `negg batch` stores in `roll.negg.json`.
  - `--preset-all-frames` (requires `--preset`, batch roll mode only; an error elsewhere): also `clear_look()` on each frame's overrides before rendering, written to the frame sidecars. Without it, frame look overrides win as `resolve` does today.
  - `negg preset list`: one line per preset, `name` then `(built-in)` or the file path; warnings from `list_presets` to stderr as `negg preset: warning: <text>`.

- [ ] **Step 1: Write the failing tests**
- `presets.rs::convert_with_a_preset_records_its_look_and_label`: sidecar settings equal default plus Rich's look and `preset: "Rich"`.
- `presets.rs::batch_preset_keeps_frame_looks_unless_all_frames`: a roll whose frame b has a grade override: without the flag b's sidecar keeps its grade; with it, b's overrides have no look fields and its settings are Rich.
- `presets.rs::preset_list_shows_builtins_and_user_presets` (sets `NEGG_PRESETS_DIR` to a temp folder holding one user preset).
- `presets.rs::unknown_preset_is_a_usage_error` (exit code as other usage errors, message lists names).
- `presets.rs::preset_all_frames_without_roll_mode_is_an_error`.
- `app_parity.rs::app_roll_preset_equals_cli_byte_for_byte`: for each scope, session `apply_roll_preset` then export roll, versus `negg batch --preset Rich [--preset-all-frames]` on a copy; `assert_same_files` on images and sidecars.

- [ ] **Step 2: Run to verify failure**: `cargo test -p negg-cli --test presets --test app_parity`
- [ ] **Step 3: Implement**; update the `--settings` help to say `--preset` applies after it
- [ ] **Step 4: Run tests and all checks**
- [ ] **Step 5: Leave for review**

---

### Task 6: Area-average downscale in linear light

**Files:**
- Create: `crates/negg-render/src/resample.rs`; export from `crates/negg-render/src/lib.rs`
- Modify: `docs/MATHS.md` step 8 (web output paragraph and constants table rows)
- Create: `docs/adr/0035-web-export.md`; add to `docs/adr/README.md`

**Interfaces:**
- Produces: `pub fn downscale_to_long_edge(image: &Image, long_edge: usize) -> Image`. When `max(w, h) <= long_edge` (or `long_edge == 0`), returns a clone. Else scale `t = long_edge / max(w, h)`, output `max(1, round(w * t))` by `max(1, round(h * t))`, the long side exactly `long_edge`. Each output pixel is the area-weighted mean of the input pixels its footprint covers (box filter with fractional edge weights), computed in `f64` per output pixel, rows in parallel with the crate's existing helpers; deterministic.

- [ ] **Step 1: Write the failing tests**
- `web_never_upscales`: 1000 x 600 at 2048 returns an equal image.
- `long_edge_is_exact_and_aspect_kept`: 6000 x 4000 gives 2048 x 1365; 4000 x 6000 gives 1365 x 2048.
- `flat_image_stays_flat`: constant `[0.3, 0.5, 0.7]` input stays that value within 1e-6.
- `integer_ratio_equals_box_mean`: 8 x 4 to long edge 4 equals the 2 x 2 means.
- `energy_is_preserved`: mean of output equals mean of input within 1e-5 for a random 997 x 613 image at 300.

- [ ] **Step 2: Run to verify failure**: `cargo test -p negg-render resample`
- [ ] **Step 3: Implement**; MATHS step 8 gains "Web output (phase 4, ADR 0035)": after geometry, `downscale_to_long_edge` at 2048, then the sRGB output transform, 8-bit JPEG quality 85, ICC embedded, `web/` subfolder; constants table rows "web long edge 2048" and "web JPEG quality 85". ADR 0035 records the constants, the filter choice (area average: no ringing, exact for integer ratios) and Eval delta "none, export-only".
- [ ] **Step 4: Run tests and all checks**
- [ ] **Step 5: Leave for review**

---

### Task 7: Web output through flows, session, CLI and FFI

**Files:**
- Modify: `crates/negg-session/src/convert.rs` (`WebOutput`, constants, `FrameOptions.web`, `Converted.web`, write path, target checks)
- Modify: `crates/negg-session/src/flow.rs` (`OutputRequest.web`, pass to frames; duplicate-output checks cover web paths)
- Modify: `crates/negg-session/src/view.rs` (`ExportRequest.web` with `#[serde(default)]`)
- Modify: `crates/negg-session/src/session.rs` (`export` passes `web`; `ExportReport.outputs` lists web files too)
- Modify: `crates/negg-cli/src/cli.rs`, `crates/negg-cli/src/convert.rs`, `crates/negg-cli/src/roll.rs` (`--web`, `--web-only`)
- Modify: `crates/negg-ffi/src/lib.rs` (`negg_export_web`, `NEGG_ABI_VERSION` 2), regenerate the header as the crate does, `crates/negg-ffi/tests/smoke.c` and `ffi.rs`
- Test: `crates/negg-cli/tests/web.rs` (new), `crates/negg-cli/tests/app_parity.rs`

**Interfaces:**
- Consumes: Task 6 `downscale_to_long_edge`.
- Produces:
  - `#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "snake_case")] pub enum WebOutput { #[default] None, Also, Only }` in `convert.rs`, re-exported from the crate root and used by `ExportRequest.web` and `OutputRequest.web`
  - `pub const WEB_LONG_EDGE: usize = 2048; pub const WEB_JPEG_QUALITY: u8 = 85; pub const WEB_SUBDIR: &str = "web";`
  - `pub fn web_output_path(input: &Path, out_dir: &Path) -> PathBuf` = `out_dir/web/<stem>.negg.jpg`
  - `Converted.web: Option<PathBuf>`; with `Only`, `Converted.output` is the web path and no original is written
  - The frame is rendered once; the web image is made from the geometry-applied render. Sidecars are written exactly as for any export.
  - `check_targets` also refuses a web path that is the source
  - CLI: `--web` gives `Also`, `--web-only` gives `Only`; the two conflict (clap `conflicts_with`)
  - FFI: `negg_export_web(s, frame: i64, format: u32, web: u32 /* 0 none, 1 also, 2 only */, out_dir, report_json) -> NeggStatus`; `negg_export` unchanged; an unknown `web` is `NeggStatus::Session`

- [ ] **Step 1: Write the failing tests**
- `web.rs::web_also_writes_both_and_web_is_srgb_2048`: convert a fixture larger than 2048 with `--format tiff16 --space adobe_rgb --web`: the TIFF is as without `--web` (byte-equal to a run without it), and `web/<stem>.negg.jpg` exists, long edge 2048, embedded ICC equals `negg_cms::profile_bytes(Srgb)`.
- `web.rs::web_only_writes_no_original`
- `web.rs::web_export_of_a_small_crop_keeps_its_size`: `--crop` leaving 900 px on the long edge gives a 900 px web JPEG.
- `web.rs::batch_web_skips_the_web_folder_next_time`: run batch `--web` twice; the second run converts the same frames and does not treat `web/` contents as inputs.
- `web.rs::web_and_web_only_conflict`.
- `app_parity.rs::app_web_export_equals_cli_byte_for_byte`: session export with `web: Also` for the roll versus `negg batch --web`, and one frame with `Only` versus `negg convert --web-only`; images, web images and sidecars equal.
- `ffi.rs::export_web_writes_the_web_copy` and an unknown `web` value giving `Session`; extend `smoke.c` with one `negg_export_web` call.
- `view.rs`: `export_request_without_web_reads_as_none` (old JSON without `web` deserialises).

- [ ] **Step 2: Run to verify failure**: `cargo test -p negg-cli --test web --test app_parity` and `cargo test -p negg-ffi`
- [ ] **Step 3: Implement**
- [ ] **Step 4: Run tests and all checks**
- [ ] **Step 5: Leave for review**

---

### Task 8: App backend commands, IPC types and the saturation slider

**Files:**
- Modify: `app/src-tauri/src/commands.rs`, `app/src-tauri/src/lib.rs` (register commands)
- Modify: `app/src/ipc/types.ts`, `app/src/ipc/commands.ts`, `app/src/ipc/__fixtures__/*` (regenerated in Task 1; add any new fixture)
- Modify: `app/src/sliders.ts` (saturation spec)
- Test: `app/src-tauri` command tests, `app/src/ipc/types.test.ts`, `app/src/ipc/commands.test.ts`, a sliders test

**Interfaces:**
- Consumes: Tasks 3, 4 and 7.
- Produces Tauri commands (snake_case, on the blocking pool like the others):
  - `list_presets() -> PresetList` (uses `presets_dir()`)
  - `save_preset(name: String, look: PrintLook) -> PresetList` (returns the refreshed list)
  - `apply_frame_preset(frame: u32, name: String) -> FrameView`
  - `frames_with_own_look() -> Vec<u32>`
  - `apply_roll_preset(name: String, scope: RollPresetScope) -> AppRollView`
  - `preset_thumbnail(frame: u32, name: String) -> Response` (JPEG bytes like `thumbnail`)
  - `export` request gains `web`
- TS: `PrintSettings.saturation?: number`, `PrintSettings.preset?: string | null`, the same on print overrides; `PrintLook`, `Preset`, `PresetList`, `RollPresetScope = "all_frames" | "frames_without_own_look"`, `WebOutput = "none" | "also" | "only"`, `ExportRequest.web`; command wrappers `listPresets`, `savePreset`, `applyFramePreset`, `framesWithOwnLook`, `applyRollPreset`, `presetThumbnail`.
- Slider: id `saturation`, label "Saturation", hint "Colour intensity; 1 leaves colours as printed", min 0, max 2, step 0.01, default 1, after the existing print sliders. Absent `saturation` in settings reads as 1.

- [ ] **Step 1: Write the failing tests**: decoder tests for the new types (fixture with and without `saturation`); command wrapper tests with the mocked invoke; slider get/set test; src-tauri tests `apply_roll_preset_command_records_one_history_entry` and `list_presets_uses_the_env_folder`.
- [ ] **Step 2: Run to verify failure**: `pnpm exec vitest run`; `cargo test` in `app/src-tauri`
- [ ] **Step 3: Implement**
- [ ] **Step 4: Run all app checks**
- [ ] **Step 5: Leave for review**

---

### Task 9: Preset UI (frame strip, cycle, save, roll picker and prompt) and export dialog

**Files:**
- Create: `app/src/components/PresetStrip.tsx`, `app/src/components/RollPresetPrompt.tsx`, `app/src/presets.ts` (pure helpers)
- Modify: `app/src/screens/FrameScreen.tsx`, `app/src/components/RollSettingsPanel.tsx`, `app/src/screens/ExportDialog.tsx`, `app/src/state/store.ts` and `reducer.ts` as needed
- Test: `app/src/presets.test.ts`, component tests beside the existing ones

**Interfaces:**
- Consumes: Task 8 commands and types.
- Produces:
  - `presetState(settings: Settings, presets: Preset[]): { name: string; modified: boolean } | null`: the label from `settings.print.preset`; `modified` when any of the five look values differs from that preset's.
  - `nextPreset(presets: Preset[], current: string | null, step: 1 | -1): Preset` (wraps; from none starts at the first).
  - Frame screen: a strip of preset thumbnails (one `presetThumbnail` per preset, cached by frame, preset name and frame generation), clicking applies at frame scope; a cycle button and the `]` (next) and `[` (previous) keys, ignored while focus is in a text field; heading shows "Rich" or "Rich (modified)"; "Save as preset" asks for a name (inline field, not a browser dialog) and calls `savePreset` with the current look.
  - Roll settings panel: a preset picker. On choose, call `framesWithOwnLook`; when empty apply with `frames_without_own_look`; else show `RollPresetPrompt`: "N frames have their own look. Apply <name> to:" with buttons "All frames" and "Only frames without their own look", plus Cancel.
  - Export dialog: a "What to export" group with two checkboxes, "Original" (checked by default; format and colour space controls apply to it) and "Web/Instagram" (help text: "JPEG, sRGB, 2048 px long edge, in a web folder"); at least one must be ticked; maps to `web` none, also or only.

- [ ] **Step 1: Write the failing tests**: `presetState` (unmodified, modified, unknown label gives the name with modified true, no label gives null); `nextPreset` wrap both ways; strip renders one item per preset and clicking calls `applyFramePreset`; `]` in a text input does not cycle; roll picker with no own-look frames applies without the prompt, with some shows it and each button sends the matching scope; export dialog maps the checkbox combinations to `web` and disables Export with neither ticked.
- [ ] **Step 2: Run to verify failure**: `pnpm exec vitest run`
- [ ] **Step 3: Implement**
- [ ] **Step 4: Run all app checks**; rebuild `negg.app` as earlier tasks did and start it for a clean launch
- [ ] **Step 5: Leave for review**

---

### Task 10: Docs, plan update and preset review sheet

**Files:**
- Modify: `IMPLEMENTATION_PLAN.md` phase 4 list: DNG line marked dropped by the owner (2026-10-25) with the reason; looks-as-data line marked parked to revisit with the owner; a new ticked line for tone presets, saturation and web export
- Modify: `docs/MATHS.md` step 8 (remove the "Linear DNG (phase 4)" bullets, say dropped)
- Modify: `README.md`, `app/README.md`, `crates/negg-cli/README.md`, `crates/negg-session/README.md`, `crates/negg-render/README.md` (`saturation`, `resample` rows), `crates/negg-ffi/README.md` (`negg_export_web`, ABI 2)
- Create (scratchpad only, not the repo): a contact sheet of each built-in preset on a few corpus frames for the owner to judge by eye

**Interfaces:** none new.

- [ ] **Step 1:** Update the docs above; describe user presets' folder and file format and the `--preset` and `--web` flags.
- [ ] **Step 2:** Render each built-in preset on three to five corpus frames with `negg convert --preset <name> --web-only --out <scratchpad>/presets/<name>` and assemble a labelled contact sheet in the scratchpad; report its absolute path for the owner.
- [ ] **Step 3:** Run `cargo doc --workspace --no-deps`, the dash check and all Rust and app checks.
- [ ] **Step 4: Leave for review**
