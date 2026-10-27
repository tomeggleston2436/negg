// SPDX-License-Identifier: Apache-2.0

//! The preset store: the built-in tone presets plus the user's `*.preset.negg.toml`
//! files in the presets folder (ADR 0034).

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use negg_io::{PRESET_SUFFIX, read_preset, write_preset};
use negg_types::roll::resolve;
use negg_types::settings::PrintSettings;
use negg_types::{
    FrameOverrides, Preset, PrintLook, RollSettings, builtin_presets, validate_preset_name,
};
use serde::{Deserialize, Serialize};

use crate::error::ConvertError;
use crate::view::RollPresetScope;

/// The environment variable that overrides the user presets folder.
pub const PRESETS_DIR_ENV: &str = "NEGG_PRESETS_DIR";

/// The user presets folder: `$NEGG_PRESETS_DIR` when set and not empty, else
/// `<config dir>/negg/presets`. `None` when neither is known.
#[must_use]
pub fn presets_dir() -> Option<PathBuf> {
    dir_from(std::env::var_os(PRESETS_DIR_ENV), dirs::config_dir())
}

fn dir_from(env: Option<OsString>, config: Option<PathBuf>) -> Option<PathBuf> {
    match env {
        Some(v) if !v.is_empty() => Some(PathBuf::from(v)),
        _ => config.map(|c| c.join("negg").join("presets")),
    }
}

/// The presets available, and what was skipped.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PresetList {
    /// Built-ins in menu order, then user presets sorted by name.
    pub presets: Vec<Preset>,
    /// How many of `presets` are built-ins (they come first), so the app
    /// can refuse a built-in name before saving.
    pub builtins: usize,
    /// One message per skipped user file, naming the file.
    pub warnings: Vec<String>,
}

fn same_name(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

fn is_preset_file(p: &Path) -> bool {
    p.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(PRESET_SUFFIX))
}

/// A preset and the user file it was read from (`None` for a built-in).
#[derive(Clone, Debug, PartialEq)]
pub struct PresetEntry {
    /// The preset.
    pub preset: Preset,
    /// The `*.preset.negg.toml` file it was read from; `None` for a
    /// built-in.
    pub file: Option<PathBuf>,
}

/// Lists the built-in presets followed by the valid user presets in `dir`
/// (sorted by name, case-insensitively). A missing folder is no user presets
/// and no warning. A file that cannot be read or validated, or whose name
/// matches a built-in or an earlier user preset (case-insensitive), is
/// skipped with a warning naming it.
#[must_use]
pub fn list_presets(dir: Option<&Path>) -> PresetList {
    let (entries, warnings) = list_preset_entries(dir);
    PresetList {
        builtins: entries.iter().filter(|e| e.file.is_none()).count(),
        presets: entries.into_iter().map(|e| e.preset).collect(),
        warnings,
    }
}

/// [`list_presets`] with the file each user preset was read from, and the
/// warnings (`negg preset list`).
#[must_use]
pub fn list_preset_entries(dir: Option<&Path>) -> (Vec<PresetEntry>, Vec<String>) {
    let (presets, skipped) = scan_presets(dir);
    (presets, skipped.into_iter().map(|s| s.message).collect())
}

/// A user file [`scan_presets`] skipped, and why.
struct Skipped {
    /// The skipped file; `None` when the folder itself could not be listed.
    file: Option<PathBuf>,
    /// The warning, naming the file once.
    message: String,
}

impl Skipped {
    /// Whether the skipped file's stem (its name without
    /// [`PRESET_SUFFIX`]) is `name` (case-insensitive) or `name`'s slug, so
    /// a lookup of `name` most likely meant this file.
    fn may_be(&self, name: &str) -> bool {
        let Some(stem) = self
            .file
            .as_deref()
            .and_then(Path::file_name)
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(PRESET_SUFFIX))
        else {
            return false;
        };
        same_name(stem, name) || stem == slug(name)
    }
}

fn scan_presets(dir: Option<&Path>) -> (Vec<PresetEntry>, Vec<Skipped>) {
    let mut presets: Vec<PresetEntry> = builtin_presets()
        .into_iter()
        .map(|preset| PresetEntry { preset, file: None })
        .collect();
    let mut warnings = Vec::new();
    let mut files: Vec<PathBuf> = Vec::new();
    if let Some(dir) = dir {
        match fs::read_dir(dir) {
            Ok(rd) => {
                files = rd
                    .filter_map(Result::ok)
                    .map(|e| e.path())
                    .filter(|p| is_preset_file(p))
                    .collect();
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => warnings.push(Skipped {
                file: None,
                message: format!("{}: cannot list presets: {e}", dir.display()),
            }),
        }
    }
    files.sort();
    let mut user: Vec<PresetEntry> = Vec::new();
    for f in files {
        match read_preset(&f) {
            // The error already names the file.
            Err(e) => warnings.push(Skipped {
                file: Some(f),
                message: format!("skipped: {e}"),
            }),
            Ok(p) => {
                if presets
                    .iter()
                    .chain(&user)
                    .any(|q| same_name(&q.preset.name, &p.name))
                {
                    warnings.push(Skipped {
                        message: format!(
                            "{}: skipped: the name {:?} is already taken",
                            f.display(),
                            p.name
                        ),
                        file: Some(f),
                    });
                } else {
                    user.push(PresetEntry {
                        preset: p,
                        file: Some(f),
                    });
                }
            }
        }
    }
    user.sort_by_key(|e| e.preset.name.to_lowercase());
    presets.extend(user);
    (presets, warnings)
}

/// A frame's overrides after a roll preset (spec "Roll preset vs frames
/// with their own look"), the one rule [`crate::Session::apply_roll_preset`]
/// and `negg batch --preset` share (Ruling 7). A frame without its own
/// look is returned as it is (it takes the roll's new look). A frame with
/// its own look: with [`RollPresetScope::AllFrames`] its look fields are
/// cleared, so it takes the roll's; with
/// [`RollPresetScope::FramesWithoutOwnLook`] the whole look it renders with
/// under the roll's print settings `old_print` (the five look fields and its resolved preset label) is
/// pinned into its overrides, so it renders exactly as before (Ruling 5);
/// a resolved label of `None` is pinned as `""`, which `resolve` reads
/// back as no label (Ruling 8).
/// Its other overrides stay in both cases.
#[must_use]
pub(crate) fn preset_frame_overrides(
    old_print: &PrintSettings,
    before: &FrameOverrides,
    scope: RollPresetScope,
) -> FrameOverrides {
    let mut after = before.clone();
    if !before.print.has_own_look() {
        return after;
    }
    match scope {
        RollPresetScope::AllFrames => after.print.clear_look(),
        RollPresetScope::FramesWithoutOwnLook => {
            // The frame's print under the old roll print, by `resolve`
            // itself; only the print part is used.
            let mut old_roll = RollSettings::default();
            old_roll.shared.print = old_print.clone();
            let now = resolve(&old_roll, before).print;
            PrintLook::of(&now).apply_to_overrides("", &mut after.print);
            after.print.preset = Some(now.preset.unwrap_or_default());
        }
    }
    after
}

/// Finds a preset. A value ending in [`PRESET_SUFFIX`] or containing a path
/// separator is read as a file; anything else is a built-in or user preset
/// name (case-insensitive).
///
/// # Errors
///
/// [`ConvertError::Usage`] for an unreadable or invalid file (naming it) or an
/// unknown name (listing the available names, and the warning for any skipped
/// user file whose stem is that name or its slug, so the user learns why it
/// was not loaded).
pub fn find_preset(name_or_path: &str, dir: Option<&Path>) -> Result<Preset, ConvertError> {
    if name_or_path.ends_with(PRESET_SUFFIX) || name_or_path.contains(['/', '\\']) {
        // The error already names the file.
        return read_preset(Path::new(name_or_path))
            .map_err(|e| ConvertError::Usage(format!("cannot use the preset file: {e}")));
    }
    let (presets, skipped) = scan_presets(dir);
    if let Some(e) = presets
        .iter()
        .find(|e| same_name(&e.preset.name, name_or_path))
    {
        return Ok(e.preset.clone());
    }
    let names: Vec<&str> = presets.iter().map(|e| e.preset.name.as_str()).collect();
    let mut msg = format!(
        "no preset named {name_or_path:?}; available: {}",
        names.join(", ")
    );
    // A skipped file that looks like the one meant: say why it was skipped.
    for s in skipped.iter().filter(|s| s.may_be(name_or_path)) {
        msg.push_str("\n  ");
        msg.push_str(&s.message);
    }
    Err(ConvertError::Usage(msg))
}

fn slug(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c.is_alphanumeric() => Some(c),
            _ => None,
        })
        .collect()
}

/// Saves `look` as the user preset `name` in `dir` (created if missing), as
/// `<slug>.preset.negg.toml`, overwriting a user preset of the same name.
/// Returns the file written.
///
/// # Errors
///
/// [`ConvertError::Usage`] for an invalid name, a built-in name or an invalid
/// look; [`ConvertError::Io`] when the folder cannot be created or the file
/// cannot be written.
pub fn save_preset(dir: &Path, name: &str, look: &PrintLook) -> Result<PathBuf, ConvertError> {
    validate_preset_name(name).map_err(|e| ConvertError::Usage(e.to_string()))?;
    if builtin_presets().iter().any(|p| same_name(&p.name, name)) {
        return Err(ConvertError::Usage(format!(
            "{name:?} is a built-in preset name; choose another"
        )));
    }
    look.validate()
        .map_err(|e| ConvertError::Usage(e.to_string()))?;
    let slug = slug(name);
    if slug.is_empty() {
        return Err(ConvertError::Usage(format!(
            "preset name {name:?} has no letters or digits"
        )));
    }
    fs::create_dir_all(dir).map_err(|source| ConvertError::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    let slug_path = dir.join(format!("{slug}{PRESET_SUFFIX}"));
    // Overwrite by name: reuse the file of an existing user preset of this name.
    let existing = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| is_preset_file(p))
        .find(|p| read_preset(p).is_ok_and(|q| same_name(&q.name, name)));
    let path = match existing {
        Some(p) => p,
        None => {
            if slug_path.exists() {
                // A read error already names the file.
                let why = match read_preset(&slug_path) {
                    Ok(q) => format!(
                        "{} already holds the preset {:?}",
                        slug_path.display(),
                        q.name
                    ),
                    Err(e) => format!("a file that cannot be read is in the way: {e}"),
                };
                return Err(ConvertError::Usage(format!(
                    "{why}; choose a different name"
                )));
            }
            slug_path
        }
    };
    let preset = Preset {
        name: name.to_owned(),
        look: *look,
    };
    write_preset(&path, &preset)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    use negg_types::{PrintLook, builtin_presets};

    fn temp(tag: &str) -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let d = std::env::temp_dir().join(format!(
            "negg-presets-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn look(grade: f32) -> PrintLook {
        PrintLook {
            grade,
            ..builtin_presets()[0].look
        }
    }

    fn names(l: &PresetList) -> Vec<String> {
        l.presets.iter().map(|p| p.name.clone()).collect()
    }

    #[test]
    fn list_without_a_folder_is_the_builtins() {
        let missing = temp("nofolder").join("absent");
        for dir in [None, Some(missing.as_path())] {
            let l = list_presets(dir);
            assert_eq!(l.presets, builtin_presets());
            assert!(l.warnings.is_empty());
        }
    }

    #[test]
    fn list_puts_user_presets_after_builtins_sorted() {
        let d = temp("sorted");
        save_preset(&d, "zebra", &look(100.0)).unwrap();
        save_preset(&d, "Apple", &look(101.0)).unwrap();
        save_preset(&d, "mango", &look(102.0)).unwrap();
        let l = list_presets(Some(&d));
        let n = names(&l);
        let b = builtin_presets().len();
        assert_eq!(
            &n[..b],
            names(&PresetList {
                presets: builtin_presets(),
                builtins: builtin_presets().len(),
                warnings: vec![]
            })
        );
        assert_eq!(&n[b..], ["Apple", "mango", "zebra"]);
        assert_eq!(l.builtins, b);
        assert!(l.warnings.is_empty());
    }

    #[test]
    fn list_skips_bad_user_presets_with_a_warning() {
        let d = temp("bad");
        fs::write(d.join("broken.preset.negg.toml"), "name = [").unwrap();
        fs::write(
            d.join("hot.preset.negg.toml"),
            "name = \"Hot\"\ngrade = 300.0\ntoe = 6.0\nshoulder = 3.0\nblack_point_compensation = false\nsaturation = 1.0\n",
        )
        .unwrap();
        // A valid file whose name is a built-in name.
        let rich = negg_types::Preset {
            name: "Rich".into(),
            look: look(100.0),
        };
        negg_io::write_preset(&d.join("rich.preset.negg.toml"), &rich).unwrap();
        let l = list_presets(Some(&d));
        assert_eq!(l.presets, builtin_presets());
        assert_eq!(l.warnings.len(), 3, "{:?}", l.warnings);
        for f in [
            "broken.preset.negg.toml",
            "hot.preset.negg.toml",
            "rich.preset.negg.toml",
        ] {
            assert_eq!(
                l.warnings.iter().filter(|w| w.contains(f)).count(),
                1,
                "{f}"
            );
        }
    }

    #[test]
    fn skipped_warning_names_the_file_once() {
        let d = temp("once");
        fs::write(
            d.join("hot.preset.negg.toml"),
            "schema_version = 1\nname = \"Hot\"\ngrade = 300.0\ntoe = 6.0\nshoulder = 3.0\nblack_point_compensation = false\nsaturation = 1.0\n",
        )
        .unwrap();
        let l = list_presets(Some(&d));
        assert_eq!(l.warnings.len(), 1, "{:?}", l.warnings);
        let w = &l.warnings[0];
        assert_eq!(w.matches("hot.preset.negg.toml").count(), 1, "{w}");
        assert_eq!(w.matches("profile error:").count(), 1, "{w}");
        assert!(w.contains("skipped") && w.contains("grade"), "{w}");
    }

    #[test]
    fn find_unknown_name_explains_a_skipped_file_of_that_name() {
        let d = temp("skipname");
        let bad = "schema_version = 1\nname = \"My Hot\"\ngrade = 300.0\ntoe = 6.0\nshoulder = 3.0\nblack_point_compensation = false\nsaturation = 1.0\n";
        // Matched by the stem (case-insensitive) and by the slug.
        fs::write(d.join("hot.preset.negg.toml"), bad).unwrap();
        fs::write(d.join("my-hot.preset.negg.toml"), bad).unwrap();
        fs::write(d.join("other.preset.negg.toml"), "x = [").unwrap();
        for (name, file) in [
            ("HOT", "hot.preset.negg.toml"),
            ("My Hot", "my-hot.preset.negg.toml"),
        ] {
            let crate::ConvertError::Usage(m) = find_preset(name, Some(&d)).unwrap_err() else {
                panic!("not usage")
            };
            assert!(m.contains("available: Standard"), "{m}");
            assert!(m.contains(file) && m.contains("grade"), "{m}");
            assert!(!m.contains("other.preset.negg.toml"), "{m}");
        }
        // No matching file: the plain message.
        let crate::ConvertError::Usage(m) = find_preset("Nonesuch", Some(&d)).unwrap_err() else {
            panic!("not usage")
        };
        assert!(!m.contains("skipped"), "{m}");
    }

    #[test]
    fn find_by_name_is_case_insensitive_and_reads_paths() {
        let d = temp("find");
        assert_eq!(find_preset("rICH", Some(&d)).unwrap().name, "Rich");
        let p = save_preset(&d, "My Look", &look(120.0)).unwrap();
        assert_eq!(find_preset("my look", Some(&d)).unwrap().look.grade, 120.0);
        let by_path = find_preset(p.to_str().unwrap(), None).unwrap();
        assert_eq!(by_path.name, "My Look");
        let e = find_preset(d.join("nope.preset.negg.toml").to_str().unwrap(), None).unwrap_err();
        assert!(
            matches!(e, crate::ConvertError::Usage(ref m) if m.contains("nope.preset.negg.toml"))
        );
        fs::write(d.join("bad.preset.negg.toml"), "x = [").unwrap();
        let e = find_preset(d.join("bad.preset.negg.toml").to_str().unwrap(), None).unwrap_err();
        assert!(
            matches!(e, crate::ConvertError::Usage(ref m) if m.contains("bad.preset.negg.toml"))
        );
    }

    #[test]
    fn find_unknown_name_lists_the_available_names() {
        let d = temp("unknown");
        save_preset(&d, "Mine", &look(100.0)).unwrap();
        let e = find_preset("Nonesuch", Some(&d)).unwrap_err();
        let crate::ConvertError::Usage(m) = e else {
            panic!("not usage")
        };
        assert!(m.contains("Nonesuch"));
        for n in ["Standard", "Soft", "Hard", "Flat", "Rich", "Linear", "Mine"] {
            assert!(m.contains(n), "{m}");
        }
    }

    #[test]
    fn save_refuses_builtin_names_and_overwrites_own() {
        let d = temp("save");
        for n in ["Rich", "rich", "STANDARD"] {
            assert!(matches!(
                save_preset(&d, n, &look(100.0)),
                Err(crate::ConvertError::Usage(_))
            ));
        }
        assert!(matches!(
            save_preset(&d, "a/b", &look(100.0)),
            Err(crate::ConvertError::Usage(_))
        ));
        assert!(matches!(
            save_preset(&d, "x", &look(300.0)),
            Err(crate::ConvertError::Usage(_))
        ));
        let p = save_preset(&d.join("sub"), "My Look!", &look(100.0)).unwrap();
        assert_eq!(p.file_name().unwrap(), "my-look.preset.negg.toml");
        let p2 = save_preset(&d.join("sub"), "My Look!", &look(110.0)).unwrap();
        assert_eq!(p, p2);
        assert_eq!(
            find_preset("my look!", Some(&d.join("sub")))
                .unwrap()
                .look
                .grade,
            110.0
        );
        assert_eq!(
            list_presets(Some(&d.join("sub"))).presets.len(),
            builtin_presets().len() + 1
        );
    }

    #[test]
    fn save_overwrites_by_name_and_refuses_a_slug_clash() {
        let d = temp("clash");
        let first = save_preset(&d, "ab", &look(100.0)).unwrap();
        let e = save_preset(&d, "Ab!", &look(110.0)).unwrap_err();
        let crate::ConvertError::Usage(m) = e else {
            panic!("not usage")
        };
        assert!(
            m.contains("ab.preset.negg.toml") && m.contains("\"ab\""),
            "{m}"
        );
        assert_eq!(find_preset("ab", Some(&d)).unwrap().look.grade, 100.0);
        // A hand-made file with another file name holds "Bar": save reuses it.
        let hand = d.join("foo.preset.negg.toml");
        let bar = negg_types::Preset {
            name: "Bar".into(),
            look: look(105.0),
        };
        negg_io::write_preset(&hand, &bar).unwrap();
        let p = save_preset(&d, "bar", &look(120.0)).unwrap();
        assert_eq!(p, hand);
        assert!(!d.join("bar.preset.negg.toml").exists());
        let l = list_presets(Some(&d));
        assert!(l.warnings.is_empty(), "{:?}", l.warnings);
        assert_eq!(find_preset("Bar", Some(&d)).unwrap().look.grade, 120.0);
        assert!(first.exists());
    }

    #[test]
    fn presets_dir_follows_the_env_variable() {
        // The only test that touches the environment.
        assert_eq!(
            dir_from(Some("/x/y".into()), Some(PathBuf::from("/cfg"))),
            Some(PathBuf::from("/x/y"))
        );
        assert_eq!(
            dir_from(None, Some(PathBuf::from("/cfg"))),
            Some(PathBuf::from("/cfg/negg/presets"))
        );
        assert_eq!(
            dir_from(Some("".into()), Some(PathBuf::from("/cfg"))),
            Some(PathBuf::from("/cfg/negg/presets"))
        );
        assert_eq!(dir_from(None, None), None);
    }
}
