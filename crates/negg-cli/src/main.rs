// SPDX-License-Identifier: Apache-2.0

//! The `negg` command-line tool: convert, batch, eval and profile.
//!
//! Owns argument parsing and orchestration only. Must not contain
//! image maths; that lives in the engine crates. Phase 0 ships `eval`.

mod cli;
mod corpus;
mod eval;

use std::process::ExitCode;

use clap::Parser;

use crate::cli::{Cli, Command};

/// Exit status for an error in the manifest, the tolerances, the baseline
/// or the command line.
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
