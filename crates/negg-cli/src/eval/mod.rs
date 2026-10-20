// SPDX-License-Identifier: Apache-2.0

//! `negg eval`: score every corpus frame and write `eval-results.json`.
//!
//! From the plan: "for each corpus frame render with default settings,
//! compute (a) CIEDE2000 against reference renders where present, (b)
//! grey-card neutrality error (mean |a*|, |b*| of the card patch), (c)
//! per-roll variance of estimated Dmin and gammas. Print a table and write
//! `eval-results.json`. Until the engine exists, this runs against the
//! reference renders themselves and reports zero; the point is the
//! plumbing."
//!
//! So in phase 0 each reference TIFF stands in for the render as well as
//! the reference: the CIEDE2000 statistics come out as zero, the grey-card
//! neutrality is measured on the first reference render, and the per-roll
//! variances are `None` because there is no estimator yet.
//!
//! With `--baseline`, the fresh report is also compared against a previous
//! `eval-results.json` using the tolerances in `eval.toml` (see
//! [`compare`] and [`tolerances`]); the comparison is printed after the
//! table and returned from [`run`], never written into the results file.

pub mod compare;
pub mod error;
pub mod metrics;
pub mod report;
pub mod tolerances;

use std::collections::HashMap;
use std::path::Path;

use negg_types::Image;

use crate::cli::EvalArgs;
use crate::corpus::{self, Frame};
pub use compare::Comparison;
pub use error::{BaselineError, EvalError};
use report::{CorpusSummary, EvalReport, FrameResult, ReferenceResult, RollResult, Summary};

/// Loads the manifest, scores every frame, writes the JSON to `args.out`
/// and prints the table to stdout unless `args.quiet`. With
/// `args.baseline`, also loads the baseline and `args.tolerances` before
/// scoring (so a bad input fails fast), compares the fresh report against
/// the baseline and prints the comparison table after the results table.
/// Returns that comparison, or `None` without `args.baseline`.
///
/// # Errors
///
/// [`EvalError::Corpus`] if the manifest fails to load or validate,
/// [`EvalError::Baseline`] or [`EvalError::Tolerances`] if those inputs
/// fail to load, [`EvalError::Decode`] if a reference render cannot be
/// read, [`EvalError::Mismatch`] if a render and reference differ in size,
/// [`EvalError::Io`] or [`EvalError::Json`] if the results cannot be
/// written.
pub fn run(args: &EvalArgs) -> Result<Option<Comparison>, EvalError> {
    let manifest = corpus::load(&args.corpus)?;
    let gate = match &args.baseline {
        Some(path) => Some((load_baseline(path)?, tolerances::load(&args.tolerances)?)),
        None => None,
    };

    let mut frames = Vec::with_capacity(manifest.frame.len());
    for frame in &manifest.frame {
        frames.push(score_frame(args, frame)?);
    }

    let report = assemble(frames, &manifest.frame);
    std::fs::write(&args.out, report::to_json(&report)?)?;
    if !args.quiet {
        print!("{}", report::render_table(&report));
    }

    let comparison = gate.map(|(baseline, tol)| compare::compare(&report, &baseline, &tol));
    if let Some(comparison) = &comparison {
        if !args.quiet {
            print!("\n{}", compare::render_table(comparison));
        }
    }
    Ok(comparison)
}

/// Reads a previous `eval-results.json` to compare against.
fn load_baseline(path: &Path) -> Result<EvalReport, EvalError> {
    let wrap = |source| EvalError::Baseline {
        path: path.to_owned(),
        source,
    };
    let text = std::fs::read_to_string(path).map_err(|e| wrap(BaselineError::Io(e)))?;
    let baseline: EvalReport =
        serde_json::from_str(&text).map_err(|e| wrap(BaselineError::Json(e)))?;
    if baseline.schema_version != report::REPORT_SCHEMA_VERSION {
        return Err(wrap(BaselineError::SchemaVersion {
            found: baseline.schema_version,
        }));
    }
    Ok(baseline)
}

/// Scores one frame. Phase 0: the reference render stands in for the
/// engine's output, so it is compared with itself.
fn score_frame(args: &EvalArgs, frame: &Frame) -> Result<FrameResult, EvalError> {
    let mut references = Vec::with_capacity(frame.references.len());
    let mut render: Option<Image> = None;
    for reference in &frame.references {
        let decoded = negg_decode::read_tiff(&args.corpus.join(&reference.file))?;
        // TODO(phase 1): render `frame.file` through the engine with
        // default settings and compare that against `decoded.image`.
        let de2000 = metrics::de2000_stats(&decoded.image, &decoded.image)
            .map_err(|e| e.for_frame(&frame.id))?;
        references.push(ReferenceResult {
            tool: reference.tool,
            de2000,
        });
        if render.is_none() {
            render = Some(decoded.image);
        }
    }
    let neutrality = match (frame.grey_card, &render) {
        (Some(card), Some(image)) => Some(metrics::neutrality(image, card)),
        _ => None,
    };
    Ok(FrameResult {
        id: frame.id.clone(),
        roll: frame.roll.clone(),
        stock: frame.stock.clone(),
        camera: frame.camera.clone(),
        light_source: frame.light_source.clone(),
        references,
        neutrality,
        estimates: None,
    })
}

/// Builds the roll entries and the summary from the per-frame results.
fn assemble(frames: Vec<FrameResult>, manifest_frames: &[Frame]) -> EvalReport {
    let corpus = CorpusSummary {
        frames: frames.len(),
        with_references: frames.iter().filter(|f| !f.references.is_empty()).count(),
        with_grey_card: manifest_frames.iter().filter(|f| f.has_grey_card).count(),
    };

    let mut roll_order: Vec<String> = Vec::new();
    let mut roll_counts: HashMap<&str, usize> = HashMap::new();
    for frame in &frames {
        if !roll_counts.contains_key(frame.roll.as_str()) {
            roll_order.push(frame.roll.clone());
        }
        *roll_counts.entry(frame.roll.as_str()).or_insert(0) += 1;
    }
    let rolls = roll_order
        .iter()
        .map(|roll| RollResult {
            roll: roll.clone(),
            frames: roll_counts[roll.as_str()],
            dmin_variance: None,
            gamma_variance: None,
        })
        .collect();

    let de_means: Vec<f32> = frames
        .iter()
        .flat_map(|f| f.references.iter().map(|r| r.de2000.mean))
        .collect();
    let de_p95s: Vec<f32> = frames
        .iter()
        .flat_map(|f| f.references.iter().map(|r| r.de2000.p95))
        .collect();
    let neutralities: Vec<f32> = frames
        .iter()
        .filter_map(|f| f.neutrality)
        .map(metrics::Neutrality::score)
        .collect();
    let summary = Summary {
        de2000_mean: mean(&de_means),
        de2000_p95_mean: mean(&de_p95s),
        neutrality_mean: mean(&neutralities),
    };

    EvalReport {
        schema_version: report::REPORT_SCHEMA_VERSION,
        corpus,
        frames,
        rolls,
        summary,
    }
}

/// Arithmetic mean, or `None` for an empty slice.
fn mean(values: &[f32]) -> Option<f32> {
    if values.is_empty() {
        None
    } else {
        let sum: f64 = values.iter().map(|&v| f64::from(v)).sum();
        Some((sum / values.len() as f64) as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corpus::Tool;
    use metrics::{DeStats, Neutrality};

    fn frame(id: &str, roll: &str, refs: usize, neutral: Option<Neutrality>) -> FrameResult {
        FrameResult {
            id: id.into(),
            roll: roll.into(),
            stock: "s".into(),
            camera: "c".into(),
            light_source: "l".into(),
            references: (0..refs)
                .map(|i| ReferenceResult {
                    tool: Tool::Negpy,
                    de2000: DeStats {
                        mean: i as f32,
                        p95: 2.0 * i as f32,
                        max: 3.0 * i as f32,
                    },
                })
                .collect(),
            neutrality: neutral,
            estimates: None,
        }
    }

    #[test]
    fn mean_of_empty_is_none() {
        assert_eq!(mean(&[]), None);
        assert_eq!(mean(&[1.0, 3.0]), Some(2.0));
    }

    #[test]
    fn assemble_groups_rolls_in_first_seen_order() {
        let frames = vec![
            frame("b-01", "b", 2, None),
            frame(
                "a-01",
                "a",
                0,
                Some(Neutrality {
                    a_mean_abs: 1.0,
                    b_mean_abs: 3.0,
                }),
            ),
            frame("b-02", "b", 1, None),
        ];
        let r = assemble(frames, &[]);
        assert_eq!(r.corpus.frames, 3);
        assert_eq!(r.corpus.with_references, 2);
        assert_eq!(r.corpus.with_grey_card, 0);
        assert_eq!(r.rolls.len(), 2);
        assert_eq!((r.rolls[0].roll.as_str(), r.rolls[0].frames), ("b", 2));
        assert_eq!((r.rolls[1].roll.as_str(), r.rolls[1].frames), ("a", 1));
        assert!(r.rolls[0].dmin_variance.is_none() && r.rolls[0].gamma_variance.is_none());
        // Reference means: 0, 1, 0 -> 1/3. p95s: 0, 2, 0 -> 2/3.
        assert!((r.summary.de2000_mean.unwrap() - 1.0 / 3.0).abs() < 1e-6);
        assert!((r.summary.de2000_p95_mean.unwrap() - 2.0 / 3.0).abs() < 1e-6);
        assert_eq!(r.summary.neutrality_mean, Some(2.0));
    }

    #[test]
    fn assemble_empty_corpus() {
        let r = assemble(vec![], &[]);
        assert_eq!(r.corpus.frames, 0);
        assert!(r.rolls.is_empty());
        assert_eq!(
            r.summary,
            Summary {
                de2000_mean: None,
                de2000_p95_mean: None,
                neutrality_mean: None
            }
        );
    }
}
