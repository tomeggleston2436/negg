// SPDX-License-Identifier: Apache-2.0

//! The `negg` command-line tool: convert, batch (roll mode), eval,
//! profile rig, profile film, lock and preset list.
//!
//! Owns argument parsing and exit statuses only. The rendering maths
//! lives in the engine crates and the flows in `negg-session`; the eval's
//! own scoring maths lives in the library's `eval` module (ADR 0028).
//!
//! Exit status: 0 on success, 1 for a conversion, measurement or I/O
//! failure (for `batch`, when any frame failed), 2 for a usage, settings,
//! profile or manifest error, 3 when `eval --baseline` finds a regression.

use std::process::ExitCode;

use clap::Parser;

use negg_cli::cli::{Cli, Command};
use negg_cli::convert::ConvertError;
use negg_cli::{eval, preset, profile, roll};

/// Exit status for an error in the manifest, the tolerances, the baseline,
/// the settings file or the command line.
const EXIT_USAGE: u8 = 2;
/// Exit status for every other failure.
const EXIT_FAILURE: u8 = 1;
/// Exit status when `--baseline` finds a regression or a missing tracked
/// case.
const EXIT_REGRESSION: u8 = 3;

fn main() -> ExitCode {
    // clap exits with status 2 itself on a usage error, and 0 for
    // `--help` and `--version`.
    let cli = Cli::parse();
    match cli.command {
        Command::Convert(args) => unit_status("negg convert", roll::run_convert(&args)),
        Command::Profile(cmd) => unit_status("negg profile", profile::run_profile(&cmd)),
        Command::Lock(args) => unit_status("negg lock", roll::run_lock(&args)),
        Command::Preset(cmd) => unit_status("negg preset", preset::run_preset(&cmd)),
        Command::Batch(args) => match roll::run_batch(&args) {
            Ok(summary) if summary.failed == 0 => ExitCode::SUCCESS,
            Ok(_) => ExitCode::from(EXIT_FAILURE),
            Err(err) => {
                eprintln!("negg batch: {err}");
                ExitCode::from(if err.is_usage_error() {
                    EXIT_USAGE
                } else {
                    EXIT_FAILURE
                })
            }
        },
        Command::Eval(args) => match eval::run(&args) {
            Ok(comparison) => {
                if comparison.is_some_and(|c| c.regressed) {
                    ExitCode::from(EXIT_REGRESSION)
                } else {
                    ExitCode::SUCCESS
                }
            }
            Err(err) => {
                eprintln!("negg eval: {err}");
                let code = if err.is_input_error() {
                    EXIT_USAGE
                } else {
                    EXIT_FAILURE
                };
                ExitCode::from(code)
            }
        },
    }
}

/// The exit status of a command that returns nothing on success, printing
/// the error prefixed with `who`.
fn unit_status(who: &str, result: Result<(), ConvertError>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{who}: {err}");
            ExitCode::from(if err.is_usage_error() {
                EXIT_USAGE
            } else {
                EXIT_FAILURE
            })
        }
    }
}
