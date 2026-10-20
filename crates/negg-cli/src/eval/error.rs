// SPDX-License-Identifier: Apache-2.0

//! The error type for `negg eval`.

use std::fmt;
use std::path::PathBuf;

use negg_decode::DecodeError;

use crate::corpus::CorpusError;
use crate::eval::report::REPORT_SCHEMA_VERSION;
use crate::eval::tolerances::TolerancesError;

/// Why an evaluation run failed.
#[derive(Debug)]
pub enum EvalError {
    /// The manifest could not be loaded or failed validation.
    Corpus(CorpusError),
    /// A reference render could not be decoded.
    Decode(DecodeError),
    /// The results file could not be written.
    Io(std::io::Error),
    /// The results could not be serialised.
    Json(serde_json::Error),
    /// A render and its reference have different dimensions.
    Mismatch {
        /// Frame id, filled in by the caller that knows it (empty when
        /// raised by the metric itself).
        id: String,
        /// Render dimensions, `(width, height)`.
        render: (usize, usize),
        /// Reference dimensions, `(width, height)`.
        reference: (usize, usize),
    },
    /// `eval.toml` could not be loaded.
    Tolerances(TolerancesError),
    /// The baseline results file could not be loaded.
    Baseline {
        /// The baseline file.
        path: PathBuf,
        /// What went wrong.
        source: BaselineError,
    },
}

/// Why a baseline `eval-results.json` could not be loaded.
#[derive(Debug)]
pub enum BaselineError {
    /// The file could not be read.
    Io(std::io::Error),
    /// The file is not valid JSON or does not match the report schema.
    Json(serde_json::Error),
    /// The file's `schema_version` is not the one this build writes.
    SchemaVersion {
        /// The version the file declares.
        found: u32,
    },
}

impl fmt::Display for BaselineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BaselineError::Io(e) => write!(f, "cannot read: {e}"),
            BaselineError::Json(e) => write!(f, "{e}"),
            BaselineError::SchemaVersion { found } => write!(
                f,
                "schema_version {found} is not supported (expected {REPORT_SCHEMA_VERSION})"
            ),
        }
    }
}

impl std::error::Error for BaselineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BaselineError::Io(e) => Some(e),
            BaselineError::Json(e) => Some(e),
            BaselineError::SchemaVersion { .. } => None,
        }
    }
}

impl EvalError {
    /// Attaches a frame id to a [`EvalError::Mismatch`] raised without one;
    /// every other variant is returned unchanged.
    pub fn for_frame(self, frame_id: &str) -> EvalError {
        match self {
            EvalError::Mismatch {
                id,
                render,
                reference,
            } if id.is_empty() => EvalError::Mismatch {
                id: frame_id.to_owned(),
                render,
                reference,
            },
            other => other,
        }
    }

    /// Whether the error is in the user's inputs (manifest, tolerances or
    /// baseline) rather than in the run itself; the binary maps these to
    /// the usage exit status.
    pub fn is_input_error(&self) -> bool {
        matches!(
            self,
            EvalError::Corpus(_) | EvalError::Tolerances(_) | EvalError::Baseline { .. }
        )
    }
}

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EvalError::Corpus(e) => write!(f, "corpus: {e}"),
            EvalError::Decode(e) => write!(f, "decode: {e}"),
            EvalError::Io(e) => write!(f, "cannot write results: {e}"),
            EvalError::Json(e) => write!(f, "cannot serialise results: {e}"),
            EvalError::Mismatch {
                id,
                render,
                reference,
            } => write!(
                f,
                "frame {id:?}: render is {}x{} but reference is {}x{}",
                render.0, render.1, reference.0, reference.1
            ),
            EvalError::Tolerances(e) => write!(f, "tolerances: {e}"),
            EvalError::Baseline { path, source } => {
                write!(f, "baseline {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for EvalError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            EvalError::Corpus(e) => Some(e),
            EvalError::Decode(e) => Some(e),
            EvalError::Io(e) => Some(e),
            EvalError::Json(e) => Some(e),
            EvalError::Mismatch { .. } => None,
            EvalError::Tolerances(e) => Some(e),
            EvalError::Baseline { source, .. } => Some(source),
        }
    }
}

impl From<CorpusError> for EvalError {
    fn from(e: CorpusError) -> Self {
        EvalError::Corpus(e)
    }
}

impl From<DecodeError> for EvalError {
    fn from(e: DecodeError) -> Self {
        EvalError::Decode(e)
    }
}

impl From<std::io::Error> for EvalError {
    fn from(e: std::io::Error) -> Self {
        EvalError::Io(e)
    }
}

impl From<serde_json::Error> for EvalError {
    fn from(e: serde_json::Error) -> Self {
        EvalError::Json(e)
    }
}

impl From<TolerancesError> for EvalError {
    fn from(e: TolerancesError) -> Self {
        EvalError::Tolerances(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn for_frame_fills_only_an_empty_mismatch_id() {
        let e = EvalError::Mismatch {
            id: String::new(),
            render: (1, 2),
            reference: (3, 4),
        }
        .for_frame("r001-01");
        assert_eq!(
            e.to_string(),
            "frame \"r001-01\": render is 1x2 but reference is 3x4"
        );
        let kept = EvalError::Mismatch {
            id: "keep".into(),
            render: (1, 1),
            reference: (1, 1),
        }
        .for_frame("other");
        assert!(kept.to_string().contains("\"keep\""));
    }

    #[test]
    fn input_errors_are_classified() {
        let baseline = EvalError::Baseline {
            path: "eval-baseline.json".into(),
            source: BaselineError::SchemaVersion { found: 9 },
        };
        assert!(baseline.is_input_error());
        assert_eq!(
            baseline.to_string(),
            "baseline eval-baseline.json: schema_version 9 is not supported (expected 1)"
        );
        let io = EvalError::Io(std::io::Error::other("disk"));
        assert!(!io.is_input_error());
    }
}
