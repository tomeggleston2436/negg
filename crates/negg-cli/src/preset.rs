// SPDX-License-Identifier: Apache-2.0

//! `negg preset list` (ADR 0034): the presets `--preset` can name.

use std::path::Path;

use negg_session::PresetEntry;

use crate::cli::PresetCommand;
use crate::convert::ConvertError;

/// One line of `negg preset list`: the name, a tab, then `(built-in)` or
/// the file the preset was read from.
pub fn format_entry(entry: &PresetEntry) -> String {
    let from = entry.file.as_deref().map_or_else(
        || "(built-in)".to_owned(),
        |f: &Path| f.display().to_string(),
    );
    format!("{}\t{from}", entry.preset.name)
}

/// Runs a `negg preset` subcommand. `list` prints one line per preset
/// ([`format_entry`]) to stdout and each skipped file's warning to stderr
/// as `negg preset: warning: <text>`.
///
/// # Errors
///
/// None at present; the signature matches the other subcommands.
pub fn run_preset(cmd: &PresetCommand) -> Result<(), ConvertError> {
    match cmd {
        PresetCommand::List => {
            let dir = negg_session::presets_dir();
            let (entries, warnings) = negg_session::list_preset_entries(dir.as_deref());
            for warning in warnings {
                eprintln!("negg preset: warning: {warning}");
            }
            for entry in &entries {
                println!("{}", format_entry(entry));
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn entries_name_their_origin() {
        let preset = negg_types::builtin_presets().remove(0);
        let builtin = PresetEntry {
            preset: preset.clone(),
            file: None,
        };
        assert_eq!(format_entry(&builtin), "Standard\t(built-in)");
        let user = PresetEntry {
            preset,
            file: Some(PathBuf::from("/p/standard.preset.negg.toml")),
        };
        assert_eq!(
            format_entry(&user),
            "Standard\t/p/standard.preset.negg.toml"
        );
    }
}
