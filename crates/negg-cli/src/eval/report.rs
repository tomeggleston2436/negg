// SPDX-License-Identifier: Apache-2.0

//! The `eval-results.json` schema and the stdout table.
//!
//! The JSON carries no timestamps or absolute paths so that two CI runs
//! on the same corpus produce byte-identical files and any difference is a
//! real change in a metric.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::corpus::Tool;
use crate::eval::metrics::{DeStats, Neutrality};

/// Schema version written to `eval-results.json`.
pub const REPORT_SCHEMA_VERSION: u32 = 1;

/// The whole results file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalReport {
    /// Always [`REPORT_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Corpus-wide counts.
    pub corpus: CorpusSummary,
    /// One entry per frame, in manifest order.
    pub frames: Vec<FrameResult>,
    /// One entry per roll, in order of first appearance.
    pub rolls: Vec<RollResult>,
    /// Corpus-wide averages.
    pub summary: Summary,
}

/// How many frames the corpus holds and how many carry each kind of data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorpusSummary {
    /// Total frames.
    pub frames: usize,
    /// Frames with at least one reference render.
    pub with_references: usize,
    /// Frames with a grey card.
    pub with_grey_card: usize,
}

/// The scores for one frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameResult {
    /// Frame id from the manifest.
    pub id: String,
    /// Roll id.
    pub roll: String,
    /// Film stock.
    pub stock: String,
    /// Scanning camera.
    pub camera: String,
    /// Backlight.
    pub light_source: String,
    /// One entry per reference render.
    pub references: Vec<ReferenceResult>,
    /// Grey-card neutrality, when the frame has a card and a render to
    /// measure it on.
    pub neutrality: Option<Neutrality>,
    /// Estimated film parameters; `None` until the estimator exists.
    pub estimates: Option<Estimates>,
}

/// CIEDE2000 against one reference render.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReferenceResult {
    /// The tool that made the reference.
    pub tool: Tool,
    /// The colour-difference statistics.
    pub de2000: DeStats,
}

/// Film parameters estimated from a frame. Reserved for the estimator;
/// nothing fills it in phase 0.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Estimates {
    /// Estimated film base density (Dmin) per channel.
    pub dmin: [f32; 3],
    /// Estimated gamma per channel.
    pub gamma: [f32; 3],
}

/// Per-roll consistency of the estimates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollResult {
    /// Roll id.
    pub roll: String,
    /// Number of frames in the roll.
    pub frames: usize,
    /// Variance of the estimated Dmin across the roll's frames; `None`
    /// until the estimator exists.
    pub dmin_variance: Option<f32>,
    /// Variance of the estimated gammas across the roll's frames; `None`
    /// until the estimator exists.
    pub gamma_variance: Option<f32>,
}

/// Corpus-wide averages. Each is `None` when nothing contributed to it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    /// Mean of the per-reference CIEDE2000 means.
    pub de2000_mean: Option<f32>,
    /// Mean of the per-reference CIEDE2000 95th percentiles.
    pub de2000_p95_mean: Option<f32>,
    /// Mean over frames with a neutrality score of `(|a*| + |b*|) / 2`.
    pub neutrality_mean: Option<f32>,
}

/// Serialises the report as pretty JSON with a trailing newline.
pub fn to_json(report: &EvalReport) -> Result<String, serde_json::Error> {
    let mut text = serde_json::to_string_pretty(report)?;
    text.push('\n');
    Ok(text)
}

/// Formats an optional metric for the table: four decimals or `-`.
pub(super) fn opt(v: Option<f32>) -> String {
    v.map_or_else(|| "-".to_owned(), |v| format!("{v:.4}"))
}

/// Renders the stdout table: one row per frame-reference pair, then one
/// row per roll, then the summary line. With no frames the table is the
/// single line `No frames in corpus`. Column widths grow to fit their
/// content; numbers are right-aligned.
pub fn render_table(report: &EvalReport) -> String {
    let mut out = String::new();
    if report.frames.is_empty() {
        out.push_str("No frames in corpus\n");
    } else {
        write_frame_table(&mut out, report);
        out.push('\n');
        write_roll_table(&mut out, report);
        out.push('\n');
    }
    let s = &report.summary;
    let _ = writeln!(
        out,
        "summary: dE mean {}  dE p95 mean {}  neutrality mean {}  ({} frames, {} with references, {} with grey card)",
        opt(s.de2000_mean),
        opt(s.de2000_p95_mean),
        opt(s.neutrality_mean),
        report.corpus.frames,
        report.corpus.with_references,
        report.corpus.with_grey_card
    );
    out
}

/// Writes rows of `cells`, padding each column to its widest entry.
/// `right` marks the columns that are right-aligned.
pub(super) fn write_aligned(out: &mut String, rows: &[Vec<String>], right: &[bool]) {
    let columns = right.len();
    let mut widths = vec![0usize; columns];
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    for row in rows {
        let mut line = String::new();
        for (i, cell) in row.iter().enumerate() {
            if i > 0 {
                line.push_str("  ");
            }
            if right[i] {
                let _ = write!(line, "{cell:>w$}", w = widths[i]);
            } else {
                let _ = write!(line, "{cell:<w$}", w = widths[i]);
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
}

fn write_frame_table(out: &mut String, report: &EvalReport) {
    let header = [
        "id",
        "stock",
        "light_source",
        "tool",
        "dE mean",
        "dE p95",
        "dE max",
        "|a*|",
        "|b*|",
    ];
    let mut rows = vec![header.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>()];
    for frame in &report.frames {
        let (a, b) = frame
            .neutrality
            .map_or((None, None), |n| (Some(n.a_mean_abs), Some(n.b_mean_abs)));
        if frame.references.is_empty() {
            rows.push(vec![
                frame.id.clone(),
                frame.stock.clone(),
                frame.light_source.clone(),
                "-".to_owned(),
                "-".to_owned(),
                "-".to_owned(),
                "-".to_owned(),
                opt(a),
                opt(b),
            ]);
        }
        for reference in &frame.references {
            rows.push(vec![
                frame.id.clone(),
                frame.stock.clone(),
                frame.light_source.clone(),
                reference.tool.name().to_owned(),
                format!("{:.4}", reference.de2000.mean),
                format!("{:.4}", reference.de2000.p95),
                format!("{:.4}", reference.de2000.max),
                opt(a),
                opt(b),
            ]);
        }
    }
    write_aligned(
        out,
        &rows,
        &[false, false, false, false, true, true, true, true, true],
    );
}

fn write_roll_table(out: &mut String, report: &EvalReport) {
    let mut rows = vec![
        ["roll", "frames", "dmin var", "gamma var"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect::<Vec<_>>(),
    ];
    for roll in &report.rolls {
        rows.push(vec![
            roll.roll.clone(),
            roll.frames.to_string(),
            opt(roll.dmin_variance),
            opt(roll.gamma_variance),
        ]);
    }
    write_aligned(out, &rows, &[false, true, true, true]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_report() -> EvalReport {
        EvalReport {
            schema_version: REPORT_SCHEMA_VERSION,
            corpus: CorpusSummary {
                frames: 0,
                with_references: 0,
                with_grey_card: 0,
            },
            frames: vec![],
            rolls: vec![],
            summary: Summary {
                de2000_mean: None,
                de2000_p95_mean: None,
                neutrality_mean: None,
            },
        }
    }

    fn one_frame_report() -> EvalReport {
        EvalReport {
            schema_version: REPORT_SCHEMA_VERSION,
            corpus: CorpusSummary {
                frames: 1,
                with_references: 1,
                with_grey_card: 1,
            },
            frames: vec![FrameResult {
                id: "r001-03".into(),
                roll: "r001".into(),
                stock: "Kodak Portra 400".into(),
                camera: "Fujifilm X-T4".into(),
                light_source: "Cinestill CS-Lite".into(),
                references: vec![ReferenceResult {
                    tool: Tool::Negpy,
                    de2000: DeStats {
                        mean: 0.0,
                        p95: 0.0,
                        max: 0.0,
                    },
                }],
                neutrality: Some(Neutrality {
                    a_mean_abs: 0.5,
                    b_mean_abs: 0.25,
                }),
                estimates: None,
            }],
            rolls: vec![RollResult {
                roll: "r001".into(),
                frames: 1,
                dmin_variance: None,
                gamma_variance: None,
            }],
            summary: Summary {
                de2000_mean: Some(0.0),
                de2000_p95_mean: Some(0.0),
                neutrality_mean: Some(0.375),
            },
        }
    }

    #[test]
    fn empty_json_matches_schema() {
        let text = to_json(&empty_report()).unwrap();
        assert!(text.ends_with('\n'));
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["schema_version"], 1);
        assert_eq!(v["corpus"]["frames"], 0);
        assert_eq!(v["frames"], serde_json::json!([]));
        assert_eq!(v["rolls"], serde_json::json!([]));
        assert!(v["summary"]["de2000_mean"].is_null());
        assert!(v["summary"]["de2000_p95_mean"].is_null());
        assert!(v["summary"]["neutrality_mean"].is_null());
    }

    #[test]
    fn one_frame_json_matches_schema() {
        let text = to_json(&one_frame_report()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        let f = &v["frames"][0];
        assert_eq!(f["id"], "r001-03");
        assert_eq!(f["references"][0]["tool"], "negpy");
        assert_eq!(f["references"][0]["de2000"]["mean"], 0.0);
        assert_eq!(f["neutrality"]["a_mean_abs"], 0.5);
        assert!(f["estimates"].is_null());
        assert!(v["rolls"][0]["dmin_variance"].is_null());
        assert!(v["rolls"][0]["gamma_variance"].is_null());
        assert_eq!(v["rolls"][0]["frames"], 1);
        assert_eq!(v["summary"]["de2000_mean"], 0.0);
        // Round trip through the typed structs too.
        let back: EvalReport = serde_json::from_str(&text).unwrap();
        assert_eq!(back.frames[0].references[0].tool, Tool::Negpy);
    }

    #[test]
    fn empty_table_says_so() {
        let t = render_table(&empty_report());
        assert!(t.starts_with("No frames in corpus\n"), "{t}");
        assert!(
            t.contains("summary: dE mean -  dE p95 mean -  neutrality mean -"),
            "{t}"
        );
    }

    #[test]
    fn one_frame_table_has_frame_roll_and_summary_rows() {
        let t = render_table(&one_frame_report());
        let lines: Vec<&str> = t.lines().collect();
        assert!(lines[0].starts_with("id"), "{t}");
        assert!(
            lines[0].contains("dE mean") && lines[0].contains("|b*|"),
            "{t}"
        );
        assert!(lines[1].starts_with("r001-03"), "{t}");
        assert!(
            lines[1].contains("negpy") && lines[1].contains("0.5000"),
            "{t}"
        );
        assert!(lines.iter().any(|l| l.starts_with("roll")), "{t}");
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("r001 ") && l.ends_with("-")),
            "{t}"
        );
        assert!(lines.last().unwrap().starts_with("summary:"), "{t}");
        assert!(!t.contains('\u{2014}') && !t.contains('\u{2013}'));
    }
}
