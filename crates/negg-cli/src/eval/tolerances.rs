// SPDX-License-Identifier: Apache-2.0

//! The `eval.toml` schema: regression tolerances and tracked cases.
//!
//! Every metric is "lower is better", so a metric regresses when
//! `current > baseline + tolerance`. The file is parsed strictly: an
//! unknown key is an error, so a typo cannot silently loosen the gate.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::corpus::Tool;

/// The `schema_version` this build understands.
pub const TOLERANCES_SCHEMA_VERSION: u32 = 1;

/// The whole of `eval.toml`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tolerances {
    /// Must equal [`TOLERANCES_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Tolerances for the summary and per-roll metrics.
    pub tolerance: ToleranceTable,
    /// Frames tracked individually; empty when the file has no `[[case]]`.
    #[serde(default)]
    pub case: Vec<TrackedCase>,
}

/// Tolerances for the corpus-wide and per-roll metrics, in the units of
/// the metric itself.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToleranceTable {
    /// Summary `de2000_mean`.
    pub de2000_mean: f32,
    /// Summary `de2000_p95_mean`.
    pub de2000_p95_mean: f32,
    /// Summary `neutrality_mean`.
    pub neutrality_mean: f32,
    /// Each roll's `dmin_variance`.
    pub dmin_variance: f32,
    /// Each roll's `gamma_variance`.
    pub gamma_variance: f32,
}

/// A frame that once rendered badly and is compared on its own metric.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrackedCase {
    /// Frame id from the corpus manifest.
    pub id: String,
    /// Which reference render's CIEDE2000 to compare. Required for the
    /// `de2000_*` metrics, ignored for `neutrality`.
    pub tool: Option<Tool>,
    /// The per-frame metric to compare.
    pub metric: CaseMetric,
    /// Allowed increase over the baseline before the case regresses.
    pub tolerance: f32,
    /// Why the frame is tracked.
    pub note: Option<String>,
}

/// The per-frame metrics a tracked case can compare.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaseMetric {
    /// `references[].de2000.mean` for the case's tool.
    De2000Mean,
    /// `references[].de2000.p95` for the case's tool.
    De2000P95,
    /// `references[].de2000.max` for the case's tool.
    De2000Max,
    /// `(neutrality.a_mean_abs + neutrality.b_mean_abs) / 2`.
    Neutrality,
}

impl CaseMetric {
    /// The metric's name as written in `eval.toml`.
    pub fn name(self) -> &'static str {
        match self {
            CaseMetric::De2000Mean => "de2000_mean",
            CaseMetric::De2000P95 => "de2000_p95",
            CaseMetric::De2000Max => "de2000_max",
            CaseMetric::Neutrality => "neutrality",
        }
    }

    /// Whether the metric is read from a reference render, and so needs
    /// the case's `tool`.
    pub fn needs_tool(self) -> bool {
        !matches!(self, CaseMetric::Neutrality)
    }
}

/// Why `eval.toml` could not be loaded.
#[derive(Debug)]
pub enum TolerancesError {
    /// The file could not be read.
    Io {
        /// The file that was being read.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
    /// The file is not valid TOML or does not match the schema.
    Parse {
        /// The file that was being parsed.
        path: PathBuf,
        /// The underlying error.
        source: toml::de::Error,
    },
    /// The file's `schema_version` is not the one this build understands.
    SchemaVersion {
        /// The file that was being parsed.
        path: PathBuf,
        /// The version the file declares.
        found: u32,
    },
    /// A tracked case compares a `de2000_*` metric but names no `tool`.
    MissingTool {
        /// The file that was being parsed.
        path: PathBuf,
        /// The case's frame id.
        id: String,
    },
}

impl fmt::Display for TolerancesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TolerancesError::Io { path, source } => {
                write!(f, "cannot read {}: {source}", path.display())
            }
            TolerancesError::Parse { path, source } => {
                write!(f, "{}: {source}", path.display())
            }
            TolerancesError::SchemaVersion { path, found } => write!(
                f,
                "{}: schema_version {found} is not supported (expected {TOLERANCES_SCHEMA_VERSION})",
                path.display()
            ),
            TolerancesError::MissingTool { path, id } => write!(
                f,
                "{}: tracked case {id:?} compares a de2000 metric but has no tool",
                path.display()
            ),
        }
    }
}

impl std::error::Error for TolerancesError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            TolerancesError::Io { source, .. } => Some(source),
            TolerancesError::Parse { source, .. } => Some(source),
            TolerancesError::SchemaVersion { .. } | TolerancesError::MissingTool { .. } => None,
        }
    }
}

/// Reads and validates an `eval.toml`.
///
/// # Errors
///
/// [`TolerancesError`] when the file cannot be read, is not valid TOML,
/// has an unknown key, declares another schema version, or has a tracked
/// case on a `de2000_*` metric without a `tool`.
pub fn load(path: &Path) -> Result<Tolerances, TolerancesError> {
    let text = std::fs::read_to_string(path).map_err(|source| TolerancesError::Io {
        path: path.to_owned(),
        source,
    })?;
    parse(&text, path)
}

/// Parses and validates the text of an `eval.toml`; `path` is only used
/// in error messages.
///
/// # Errors
///
/// As for [`load`], except that nothing is read from disk.
pub fn parse(text: &str, path: &Path) -> Result<Tolerances, TolerancesError> {
    let tolerances: Tolerances = toml::from_str(text).map_err(|source| TolerancesError::Parse {
        path: path.to_owned(),
        source,
    })?;
    if tolerances.schema_version != TOLERANCES_SCHEMA_VERSION {
        return Err(TolerancesError::SchemaVersion {
            path: path.to_owned(),
            found: tolerances.schema_version,
        });
    }
    if let Some(case) = tolerances
        .case
        .iter()
        .find(|c| c.metric.needs_tool() && c.tool.is_none())
    {
        return Err(TolerancesError::MissingTool {
            path: path.to_owned(),
            id: case.id.clone(),
        });
    }
    Ok(tolerances)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The repository's own `eval.toml`.
    const REPO_EVAL_TOML: &str = include_str!("../../../../eval.toml");

    fn parse_str(text: &str) -> Result<Tolerances, TolerancesError> {
        parse(text, Path::new("eval.toml"))
    }

    #[test]
    fn repo_eval_toml_parses_with_the_five_tolerances() {
        let t = parse_str(REPO_EVAL_TOML).unwrap();
        assert_eq!(t.schema_version, 1);
        assert_eq!(
            t.tolerance,
            ToleranceTable {
                de2000_mean: 0.10,
                de2000_p95_mean: 0.20,
                neutrality_mean: 0.10,
                dmin_variance: 0.005,
                gamma_variance: 0.010,
            }
        );
        assert!(t.case.is_empty());
    }

    #[test]
    fn repo_eval_toml_loads_from_disk() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../eval.toml");
        let t = load(&path).unwrap();
        assert_eq!(t.tolerance.de2000_mean, 0.10);
    }

    #[test]
    fn unknown_key_fails_to_parse() {
        let text = format!("{REPO_EVAL_TOML}\n[tolerance.extra]\nfoo = 1\n");
        let err = parse_str(&text).unwrap_err();
        assert!(matches!(err, TolerancesError::Parse { .. }), "{err}");

        let text = REPO_EVAL_TOML.replace("de2000_mean = 0.10", "de2000_meen = 0.10");
        let err = parse_str(&text).unwrap_err();
        assert!(err.to_string().contains("de2000_meen"), "{err}");
    }

    #[test]
    fn missing_tolerance_fails_to_parse() {
        let text = REPO_EVAL_TOML.replace("gamma_variance = 0.010", "");
        let err = parse_str(&text).unwrap_err();
        assert!(err.to_string().contains("gamma_variance"), "{err}");
    }

    #[test]
    fn wrong_schema_version_is_an_error() {
        let text = REPO_EVAL_TOML.replace("schema_version = 1", "schema_version = 2");
        let err = parse_str(&text).unwrap_err();
        assert!(
            matches!(err, TolerancesError::SchemaVersion { found: 2, .. }),
            "{err}"
        );
    }

    #[test]
    fn tracked_cases_parse() {
        let text = format!(
            "{REPO_EVAL_TOML}\n[[case]]\nid = \"r001-03\"\ntool = \"negpy\"\nmetric = \"de2000_mean\"\ntolerance = 0.5\nnote = \"cast\"\n\n[[case]]\nid = \"r002-01\"\nmetric = \"neutrality\"\ntolerance = 0.25\n"
        );
        let t = parse_str(&text).unwrap();
        assert_eq!(
            t.case,
            vec![
                TrackedCase {
                    id: "r001-03".into(),
                    tool: Some(Tool::Negpy),
                    metric: CaseMetric::De2000Mean,
                    tolerance: 0.5,
                    note: Some("cast".into()),
                },
                TrackedCase {
                    id: "r002-01".into(),
                    tool: None,
                    metric: CaseMetric::Neutrality,
                    tolerance: 0.25,
                    note: None,
                },
            ]
        );
    }

    #[test]
    fn de2000_case_without_tool_is_an_error() {
        let text = format!(
            "{REPO_EVAL_TOML}\n[[case]]\nid = \"r001-03\"\nmetric = \"de2000_max\"\ntolerance = 0.5\n"
        );
        let err = parse_str(&text).unwrap_err();
        assert!(matches!(err, TolerancesError::MissingTool { .. }), "{err}");
        assert!(err.to_string().contains("r001-03"), "{err}");
    }

    #[test]
    fn unknown_case_metric_or_key_fails() {
        let text = format!(
            "{REPO_EVAL_TOML}\n[[case]]\nid = \"x\"\ntool = \"negpy\"\nmetric = \"de2000_median\"\ntolerance = 0.5\n"
        );
        assert!(parse_str(&text).is_err());
        let text = format!(
            "{REPO_EVAL_TOML}\n[[case]]\nid = \"x\"\ntool = \"negpy\"\nmetric = \"de2000_mean\"\ntolerance = 0.5\nseverity = 3\n"
        );
        assert!(parse_str(&text).is_err());
    }

    #[test]
    fn case_metric_names_round_trip() {
        for (m, name) in [
            (CaseMetric::De2000Mean, "de2000_mean"),
            (CaseMetric::De2000P95, "de2000_p95"),
            (CaseMetric::De2000Max, "de2000_max"),
            (CaseMetric::Neutrality, "neutrality"),
        ] {
            assert_eq!(m.name(), name);
            let parsed: CaseMetric = serde_json::from_str(&format!("\"{name}\"")).unwrap();
            assert_eq!(parsed, m);
        }
        assert!(CaseMetric::De2000Mean.needs_tool());
        assert!(!CaseMetric::Neutrality.needs_tool());
    }
}
