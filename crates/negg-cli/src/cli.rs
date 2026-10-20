// SPDX-License-Identifier: Apache-2.0

//! Command-line definition for the `negg` binary.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// The `negg` film negative converter.
#[derive(Debug, Parser)]
#[command(name = "negg", version, about)]
pub struct Cli {
    /// The subcommand to run.
    #[command(subcommand)]
    pub command: Command,
}

/// Top-level subcommands.
///
/// `Convert`, `Batch` and `Profile` arrive with the engine in later
/// phases; they are deliberately not stubbed here.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Render every corpus frame and score it against the reference renders.
    Eval(EvalArgs),
}

/// Arguments for `negg eval`.
#[derive(Debug, Args)]
pub struct EvalArgs {
    /// Corpus directory containing `manifest.toml`.
    #[arg(long, default_value = "corpus")]
    pub corpus: PathBuf,
    /// Where to write the results JSON.
    #[arg(long, default_value = "eval-results.json")]
    pub out: PathBuf,
    /// Write the JSON but do not print the table.
    #[arg(long)]
    pub quiet: bool,
    /// A previous results JSON to compare against. Any metric that worsens
    /// by more than its tolerance makes the command exit with status 3.
    #[arg(long, value_name = "FILE")]
    pub baseline: Option<PathBuf>,
    /// Tolerances and tracked cases for the baseline comparison.
    #[arg(long, value_name = "FILE", default_value = "eval.toml")]
    pub tolerances: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eval_defaults() {
        let cli = Cli::try_parse_from(["negg", "eval"]).unwrap();
        let Command::Eval(args) = cli.command;
        assert_eq!(args.corpus, PathBuf::from("corpus"));
        assert_eq!(args.out, PathBuf::from("eval-results.json"));
        assert!(!args.quiet);
        assert_eq!(args.baseline, None);
        assert_eq!(args.tolerances, PathBuf::from("eval.toml"));
    }

    #[test]
    fn eval_flags() {
        let cli = Cli::try_parse_from([
            "negg",
            "eval",
            "--corpus",
            "c",
            "--out",
            "o.json",
            "--quiet",
            "--baseline",
            "b.json",
            "--tolerances",
            "t.toml",
        ])
        .unwrap();
        let Command::Eval(args) = cli.command;
        assert_eq!(args.corpus, PathBuf::from("c"));
        assert_eq!(args.out, PathBuf::from("o.json"));
        assert!(args.quiet);
        assert_eq!(args.baseline, Some(PathBuf::from("b.json")));
        assert_eq!(args.tolerances, PathBuf::from("t.toml"));
    }

    #[test]
    fn baseline_needs_a_value() {
        assert!(Cli::try_parse_from(["negg", "eval", "--baseline"]).is_err());
    }

    #[test]
    fn unknown_subcommand_is_rejected() {
        assert!(Cli::try_parse_from(["negg", "frobnicate"]).is_err());
    }
}
