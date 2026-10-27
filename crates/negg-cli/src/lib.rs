// SPDX-License-Identifier: Apache-2.0

//! The library behind the `negg` command-line tool: argument types, the
//! printing of `convert`, `batch`, `lock` and `preset list`, `profile rig` and
//! `profile film`, the corpus manifest and `eval`. The convert, batch,
//! roll and lock flows themselves live in `negg-session`,
//! re-exported from [`convert`] and [`roll`] so that callers keep their
//! paths. The binary in `main.rs` parses arguments and maps results to
//! exit statuses; the library is public so that integration tests (the
//! Gate 1 timing in `tests/perf.rs`) can call [`convert::convert_frame`]
//! directly.
//!
//! Owns orchestration and printing only. Must not contain image maths for rendering;
//! that lives in the engine crates. The one exception is the eval's own
//! scoring maths (reference alignment, metrics, comparison), which lives
//! in `eval/` (ADR 0028).

pub mod cli;
pub mod convert;
pub mod corpus;
pub mod eval;
pub mod preset;
pub mod profile;
pub mod roll;
