// SPDX-License-Identifier: MPL-2.0

//! Tone presets: a [`PrintLook`] is the five print parameters a preset may
//! set (grade, toe, shoulder, black point compensation and saturation; steps
//! 5 and 7 of `docs/MATHS.md`), and a [`Preset`] is a named look (ADR 0034).
//!
//! A look never touches exposure, CMY, paper constants, the film estimates,
//! geometry, profiles or the output space. No maths is done here.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::roll::PrintOverrides;
use crate::settings::{PrintSettings, Settings};

/// The longest preset name, in characters.
const MAX_NAME_CHARS: usize = 40;

/// The print parameters a tone preset sets (steps 5 and 7 of
/// `docs/MATHS.md`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrintLook {
    /// `print.grade`, ISO-R style, 50 to 180.
    pub grade: f32,
    /// `print.toe`, the shadow steepness `a_sh`.
    pub toe: f32,
    /// `print.shoulder`, the highlight steepness `a_hl`.
    pub shoulder: f32,
    /// `print.black_point_compensation`.
    pub black_point_compensation: bool,
    /// `print.saturation`, 0 to 2.
    pub saturation: f32,
}

/// A named [`PrintLook`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    /// The display name, valid per [`validate_preset_name`].
    pub name: String,
    /// The look the preset applies.
    pub look: PrintLook,
}

impl PrintLook {
    /// The look currently held by `print`.
    #[must_use]
    pub fn of(print: &PrintSettings) -> PrintLook {
        PrintLook {
            grade: print.grade,
            toe: print.toe,
            shoulder: print.shoulder,
            black_point_compensation: print.black_point_compensation,
            saturation: print.saturation,
        }
    }

    /// Sets the five look fields on `print` and labels it `name`. Nothing
    /// else changes.
    pub fn apply_to_settings(&self, name: &str, print: &mut PrintSettings) {
        print.grade = self.grade;
        print.toe = self.toe;
        print.shoulder = self.shoulder;
        print.black_point_compensation = self.black_point_compensation;
        print.saturation = self.saturation;
        print.preset = Some(name.to_owned());
    }

    /// Sets the five look fields and the label on a frame's overrides.
    /// Nothing else changes.
    pub fn apply_to_overrides(&self, name: &str, o: &mut PrintOverrides) {
        o.grade = Some(self.grade);
        o.toe = Some(self.toe);
        o.shoulder = Some(self.shoulder);
        o.black_point_compensation = Some(self.black_point_compensation);
        o.saturation = Some(self.saturation);
        o.preset = Some(name.to_owned());
    }

    /// Checks the look against the settings ranges, by applying it to a
    /// default [`Settings`] and validating that.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] naming the offending field.
    pub fn validate(&self) -> Result<()> {
        let mut s = Settings::default();
        self.apply_to_settings("check", &mut s.print);
        s.validate()
    }
}

impl PrintOverrides {
    /// Whether any look field (grade, toe, shoulder, black point
    /// compensation, saturation or the preset label) is overridden.
    #[must_use]
    pub fn has_own_look(&self) -> bool {
        self.grade.is_some()
            || self.toe.is_some()
            || self.shoulder.is_some()
            || self.black_point_compensation.is_some()
            || self.saturation.is_some()
            || self.preset.is_some()
    }

    /// Clears every look field so the frame inherits the roll's look again.
    /// Exposure, CMY and the paper constants are kept.
    pub fn clear_look(&mut self) {
        self.grade = None;
        self.toe = None;
        self.shoulder = None;
        self.black_point_compensation = None;
        self.saturation = None;
        self.preset = None;
    }
}

/// Checks a preset name: trimmed, 1 to 40 characters, no `/` or `\`, no
/// leading `.` and no control characters.
///
/// # Errors
///
/// [`Error::Invalid`] naming the rule.
pub fn validate_preset_name(name: &str) -> Result<()> {
    let bad = |why: &str| Err(Error::Invalid(format!("preset name {name:?} {why}")));
    if name.trim() != name {
        return bad("must not start or end with whitespace");
    }
    let chars = name.chars().count();
    if chars == 0 {
        return bad("must not be empty");
    }
    if chars > MAX_NAME_CHARS {
        return bad("must be at most 40 characters");
    }
    if name.contains(['/', '\\']) {
        return bad("must not contain / or \\");
    }
    if name.starts_with('.') {
        return bad("must not start with a dot");
    }
    if name.chars().any(char::is_control) {
        return bad("must not contain control characters");
    }
    Ok(())
}

/// The built-in presets, in menu order. The values were chosen by eye on the
/// corpus and are provisional (ADR 0034).
#[must_use]
pub fn builtin_presets() -> Vec<Preset> {
    let p = |name: &str, grade, toe, shoulder, black_point_compensation, saturation| Preset {
        name: name.to_owned(),
        look: PrintLook {
            grade,
            toe,
            shoulder,
            black_point_compensation,
            saturation,
        },
    };
    vec![
        p("Standard", 115.0, 6.0, 3.0, false, 1.0),
        p("Soft", 95.0, 4.0, 2.0, false, 0.95),
        p("Hard", 135.0, 8.0, 4.0, false, 1.05),
        p("Flat", 85.0, 3.0, 1.5, false, 0.9),
        p("Rich", 125.0, 8.0, 3.0, true, 1.1),
        p("Linear", 115.0, 12.0, 6.0, false, 1.0),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_are_six_valid_named_looks() {
        let presets = builtin_presets();
        let names: Vec<&str> = presets.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            ["Standard", "Soft", "Hard", "Flat", "Rich", "Linear"]
        );
        for p in &presets {
            p.look.validate().unwrap();
            validate_preset_name(&p.name).unwrap();
        }
    }

    #[test]
    fn standard_is_the_default_look() {
        assert_eq!(
            builtin_presets()[0].look,
            PrintLook::of(&PrintSettings::default())
        );
    }

    fn start() -> PrintOverrides {
        PrintOverrides {
            exposure: Some(0.2),
            cmy: Some([0.01, 0.0, -0.02]),
            paper_dmin: Some(0.07),
            ..PrintOverrides::default()
        }
    }

    fn rich() -> Preset {
        builtin_presets()
            .into_iter()
            .find(|p| p.name == "Rich")
            .unwrap()
    }

    #[test]
    fn apply_to_overrides_sets_only_look_fields() {
        let mut o = start();
        let rich = rich();
        rich.look.apply_to_overrides("Rich", &mut o);
        assert_eq!(o.exposure, Some(0.2));
        assert_eq!(o.cmy, Some([0.01, 0.0, -0.02]));
        assert_eq!(o.paper_dmin, Some(0.07));
        assert_eq!(o.paper_dmax, None);
        assert_eq!(o.grade, Some(rich.look.grade));
        assert_eq!(o.toe, Some(rich.look.toe));
        assert_eq!(o.shoulder, Some(rich.look.shoulder));
        assert_eq!(o.black_point_compensation, Some(true));
        assert_eq!(o.saturation, Some(rich.look.saturation));
        assert_eq!(o.preset.as_deref(), Some("Rich"));
    }

    #[test]
    fn apply_to_settings_sets_only_look_fields() {
        let mut print = PrintSettings {
            exposure: 0.3,
            ..PrintSettings::default()
        };
        let rich = rich();
        rich.look.apply_to_settings("Rich", &mut print);
        assert_eq!(PrintLook::of(&print), rich.look);
        assert_eq!(print.exposure, 0.3);
        assert_eq!(print.preset.as_deref(), Some("Rich"));
    }

    #[test]
    fn clear_look_keeps_corrections() {
        let mut o = start();
        rich().look.apply_to_overrides("Rich", &mut o);
        assert!(o.has_own_look());
        o.clear_look();
        assert!(!o.has_own_look());
        assert_eq!(o.exposure, Some(0.2));
        assert_eq!(o.cmy, Some([0.01, 0.0, -0.02]));
        assert_eq!(o.paper_dmin, Some(0.07));
    }

    #[test]
    fn preset_names_are_validated() {
        for bad in [
            "",
            "   ",
            "a/b",
            "a\\b",
            ".hidden",
            "tab\there",
            &"x".repeat(41),
        ] {
            assert!(validate_preset_name(bad).is_err(), "{bad:?}");
        }
        validate_preset_name("My warm").unwrap();
        validate_preset_name(&"x".repeat(40)).unwrap();
    }

    #[test]
    fn out_of_range_look_is_invalid() {
        let mut look = builtin_presets()[0].look;
        look.grade = 200.0;
        assert!(look.validate().is_err());
    }
}
