// SPDX-License-Identifier: Apache-2.0

//! The library behind the `negg` command-line tool: argument types,
//! `convert`, `batch` and roll mode, `lock`, `profile rig` and
//! `profile film`, the corpus manifest and `eval`. The binary in
//! `main.rs` parses arguments and maps results to exit statuses; the
//! library is public so that integration tests (the Gate 1 timing in
//! `tests/perf.rs`) can call [`convert::convert_frame`] directly.
//!
//! Owns orchestration only. Must not contain image maths; that lives in
//! the engine crates.

pub mod cli;
pub mod convert;
pub mod corpus;
pub mod eval;
pub mod profile;
pub mod roll;
