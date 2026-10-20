// SPDX-License-Identifier: Apache-2.0

//! The regression gate: compares a fresh [`EvalReport`] against a baseline
//! with the tolerances from `eval.toml`.
//!
//! Every metric is "lower is better". A metric regresses when
//! `current > baseline + tolerance`; when either side is null the row is
//! reported as `n/a` and does not count. Tracked cases are compared on
//! their own per-frame metric, and a case whose frame or reference has
//! disappeared from the current report fails the gate outright, because a
//! silently dropped frame is exactly what the case exists to catch.
//!
//! [`compare`] is pure so it can be tested without files; the command in
//! `mod.rs` loads the inputs and prints [`render_table`].

use std::fmt::Write as _;

use crate::eval::metrics::DeStats;
use crate::eval::report::{self, EvalReport, FrameResult};
use crate::eval::tolerances::{CaseMetric, Tolerances, TrackedCase};

/// The verdict for one row of the comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Both sides present and `current <= baseline + tolerance`.
    Ok,
    /// Both sides present and `current > baseline + tolerance`.
    Regression,
    /// One side is null; nothing to compare.
    NotAvailable,
    /// A tracked case whose frame or reference is absent from the current
    /// report. Counts as a failure.
    Missing,
}

impl Status {
    /// The label printed in the status column.
    pub fn label(self) -> &'static str {
        match self {
            Status::Ok => "OK",
            Status::Regression => "REGRESSION",
            Status::NotAvailable => "n/a",
            Status::Missing => "MISSING",
        }
    }
}

/// One compared metric.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// What was compared, as printed in the table, for example
    /// `de2000_mean`, `roll r001 dmin_variance` or
    /// `case r001-03 negpy de2000_mean`.
    pub metric: String,
    /// The frame id when the row is a tracked case.
    pub case: Option<String>,
    /// The baseline value, or `None` when null or absent.
    pub baseline: Option<f32>,
    /// The current value, or `None` when null or absent.
    pub current: Option<f32>,
    /// The allowed increase.
    pub tolerance: f32,
    /// The verdict.
    pub status: Status,
}

impl Row {
    /// `current - baseline`, when both are present.
    pub fn delta(&self) -> Option<f32> {
        Some(self.current? - self.baseline?)
    }
}

/// The whole comparison.
#[derive(Debug, Clone, PartialEq)]
pub struct Comparison {
    /// Summary metrics, then per-roll metrics, then tracked cases.
    pub rows: Vec<Row>,
    /// `true` when any row is a [`Status::Regression`] or a
    /// [`Status::Missing`]; the command then exits with status 3.
    pub regressed: bool,
}

/// Compares `current` against `baseline` with the tolerances in `tol`.
pub fn compare(current: &EvalReport, baseline: &EvalReport, tol: &Tolerances) -> Comparison {
    let t = &tol.tolerance;
    let mut rows = vec![
        metric_row(
            "de2000_mean",
            baseline.summary.de2000_mean,
            current.summary.de2000_mean,
            t.de2000_mean,
        ),
        metric_row(
            "de2000_p95_mean",
            baseline.summary.de2000_p95_mean,
            current.summary.de2000_p95_mean,
            t.de2000_p95_mean,
        ),
        metric_row(
            "neutrality_mean",
            baseline.summary.neutrality_mean,
            current.summary.neutrality_mean,
            t.neutrality_mean,
        ),
    ];
    for roll in &current.rolls {
        let base = baseline.rolls.iter().find(|r| r.roll == roll.roll);
        rows.push(metric_row(
            format!("roll {} dmin_variance", roll.roll),
            base.and_then(|b| b.dmin_variance),
            roll.dmin_variance,
            t.dmin_variance,
        ));
        rows.push(metric_row(
            format!("roll {} gamma_variance", roll.roll),
            base.and_then(|b| b.gamma_variance),
            roll.gamma_variance,
            t.gamma_variance,
        ));
    }
    for case in &tol.case {
        rows.push(case_row(current, baseline, case));
    }
    let regressed = rows
        .iter()
        .any(|r| matches!(r.status, Status::Regression | Status::Missing));
    Comparison { rows, regressed }
}

/// Applies the one rule: regression when `current > baseline + tolerance`,
/// `n/a` when either side is missing.
fn metric_row(
    metric: impl Into<String>,
    baseline: Option<f32>,
    current: Option<f32>,
    tolerance: f32,
) -> Row {
    let status = match (baseline, current) {
        (Some(b), Some(c)) if c > b + tolerance => Status::Regression,
        (Some(_), Some(_)) => Status::Ok,
        _ => Status::NotAvailable,
    };
    Row {
        metric: metric.into(),
        case: None,
        baseline,
        current,
        tolerance,
        status,
    }
}

/// What a report holds for a tracked case.
enum Lookup {
    /// The frame, or the reference for the case's tool, is not in the
    /// report.
    Missing,
    /// The frame and reference are present; the value is `None` only
    /// when the metric itself is null (a frame without a grey card).
    Value(Option<f32>),
}

fn lookup(report: &EvalReport, case: &TrackedCase) -> Lookup {
    let Some(frame) = report.frames.iter().find(|f| f.id == case.id) else {
        return Lookup::Missing;
    };
    case_value(frame, case)
}

fn case_value(frame: &FrameResult, case: &TrackedCase) -> Lookup {
    let pick: fn(&DeStats) -> f32 = match case.metric {
        CaseMetric::Neutrality => {
            return Lookup::Value(frame.neutrality.map(|n| n.score()));
        }
        CaseMetric::De2000Mean => |d| d.mean,
        CaseMetric::De2000P95 => |d| d.p95,
        CaseMetric::De2000Max => |d| d.max,
    };
    match frame.references.iter().find(|r| Some(r.tool) == case.tool) {
        Some(reference) => Lookup::Value(Some(pick(&reference.de2000))),
        None => Lookup::Missing,
    }
}

fn case_row(current: &EvalReport, baseline: &EvalReport, case: &TrackedCase) -> Row {
    let metric = match case.tool.filter(|_| case.metric.needs_tool()) {
        Some(tool) => format!("case {} {} {}", case.id, tool, case.metric.name()),
        None => format!("case {} {}", case.id, case.metric.name()),
    };
    let base = match lookup(baseline, case) {
        Lookup::Missing => None,
        Lookup::Value(v) => v,
    };
    let mut row = match lookup(current, case) {
        Lookup::Missing => Row {
            metric,
            case: None,
            baseline: base,
            current: None,
            tolerance: case.tolerance,
            status: Status::Missing,
        },
        Lookup::Value(cur) => metric_row(metric, base, cur, case.tolerance),
    };
    row.case = Some(case.id.clone());
    row
}

/// Renders the comparison table (metric, baseline, current, delta,
/// tolerance, status), one line per missing tracked case, and the verdict
/// line `regression gate: OK` or `regression gate: FAILED (...)`.
pub fn render_table(comparison: &Comparison) -> String {
    let mut out = String::new();
    let mut rows = vec![
        [
            "metric",
            "baseline",
            "current",
            "delta",
            "tolerance",
            "status",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect::<Vec<_>>(),
    ];
    for row in &comparison.rows {
        rows.push(vec![
            row.metric.clone(),
            report::opt(row.baseline),
            report::opt(row.current),
            row.delta()
                .map_or_else(|| "-".to_owned(), |d| format!("{d:+.4}")),
            format!("{:.4}", row.tolerance),
            row.status.label().to_owned(),
        ]);
    }
    report::write_aligned(&mut out, &rows, &[false, true, true, true, true, false]);

    let mut regressions = 0usize;
    let mut missing = 0usize;
    for row in &comparison.rows {
        match row.status {
            Status::Regression => regressions += 1,
            Status::Missing => {
                missing += 1;
                let id = row.case.as_deref().unwrap_or(row.metric.as_str());
                let _ = writeln!(out, "tracked case {id} missing from the current report");
            }
            Status::Ok | Status::NotAvailable => {}
        }
    }
    if comparison.regressed {
        let _ = writeln!(
            out,
            "regression gate: FAILED ({regressions} regressed, {missing} missing tracked case{})",
            if missing == 1 { "" } else { "s" }
        );
    } else {
        out.push_str("regression gate: OK\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corpus::Tool;
    use crate::eval::metrics::Neutrality;
    use crate::eval::report::{
        CorpusSummary, REPORT_SCHEMA_VERSION, ReferenceResult, RollResult, Summary,
    };
    use crate::eval::tolerances::ToleranceTable;

    fn tolerances() -> Tolerances {
        Tolerances {
            schema_version: 1,
            tolerance: ToleranceTable {
                de2000_mean: 0.1,
                de2000_p95_mean: 0.2,
                neutrality_mean: 0.1,
                dmin_variance: 0.005,
                gamma_variance: 0.01,
            },
            case: vec![],
        }
    }

    fn case(id: &str, tool: Option<Tool>, metric: CaseMetric, tolerance: f32) -> TrackedCase {
        TrackedCase {
            id: id.into(),
            tool,
            metric,
            tolerance,
            note: None,
        }
    }

    fn frame(id: &str, roll: &str, de_mean: f32, neutral: Option<f32>) -> FrameResult {
        FrameResult {
            id: id.into(),
            roll: roll.into(),
            stock: "s".into(),
            camera: "c".into(),
            light_source: "l".into(),
            references: vec![ReferenceResult {
                tool: Tool::Negpy,
                de2000: DeStats {
                    mean: de_mean,
                    p95: de_mean * 2.0,
                    max: de_mean * 3.0,
                },
            }],
            neutrality: neutral.map(|v| Neutrality {
                a_mean_abs: v,
                b_mean_abs: v,
            }),
            estimates: None,
        }
    }

    fn report(summary: Summary, rolls: Vec<RollResult>, frames: Vec<FrameResult>) -> EvalReport {
        EvalReport {
            schema_version: REPORT_SCHEMA_VERSION,
            corpus: CorpusSummary {
                frames: frames.len(),
                with_references: frames.len(),
                with_grey_card: 0,
            },
            frames,
            rolls,
            summary,
        }
    }

    fn summary_only(de: Option<f32>) -> EvalReport {
        report(
            Summary {
                de2000_mean: de,
                de2000_p95_mean: None,
                neutrality_mean: None,
            },
            vec![],
            vec![],
        )
    }

    fn roll(name: &str, dmin: Option<f32>, gamma: Option<f32>) -> RollResult {
        RollResult {
            roll: name.into(),
            frames: 1,
            dmin_variance: dmin,
            gamma_variance: gamma,
        }
    }

    fn row<'a>(c: &'a Comparison, metric: &str) -> &'a Row {
        c.rows
            .iter()
            .find(|r| r.metric == metric)
            .unwrap_or_else(|| panic!("no row {metric:?} in {c:?}"))
    }

    #[test]
    fn regression_when_current_exceeds_baseline_plus_tolerance() {
        let c = compare(
            &summary_only(Some(1.2)),
            &summary_only(Some(1.0)),
            &tolerances(),
        );
        let r = row(&c, "de2000_mean");
        assert_eq!(r.status, Status::Regression);
        assert!((r.delta().unwrap() - 0.2).abs() < 1e-6);
        assert!(c.regressed);
        assert_eq!(c.rows.len(), 3);
    }

    #[test]
    fn no_regression_at_exactly_baseline_plus_tolerance() {
        let baseline = 1.0f32;
        let tol = tolerances();
        let current = baseline + tol.tolerance.de2000_mean;
        let c = compare(
            &summary_only(Some(current)),
            &summary_only(Some(baseline)),
            &tol,
        );
        assert_eq!(row(&c, "de2000_mean").status, Status::Ok);
        assert!(!c.regressed);
        // And an improvement is OK too.
        let c = compare(&summary_only(Some(0.5)), &summary_only(Some(1.0)), &tol);
        assert_eq!(row(&c, "de2000_mean").status, Status::Ok);
        assert!(!c.regressed);
    }

    #[test]
    fn null_on_either_side_is_not_available_and_not_a_regression() {
        let tol = tolerances();
        for (cur, base) in [(None, Some(0.0)), (Some(100.0), None), (None, None)] {
            let c = compare(&summary_only(cur), &summary_only(base), &tol);
            let r = row(&c, "de2000_mean");
            assert_eq!(r.status, Status::NotAvailable, "{cur:?} vs {base:?}");
            assert_eq!(r.delta(), None);
            assert!(!c.regressed);
        }
        let empty = compare(&summary_only(None), &summary_only(None), &tol);
        assert!(empty.rows.iter().all(|r| r.status == Status::NotAvailable));
    }

    #[test]
    fn rolls_are_compared_by_name() {
        let tol = tolerances();
        let current = report(
            Summary {
                de2000_mean: None,
                de2000_p95_mean: None,
                neutrality_mean: None,
            },
            vec![
                roll("r001", Some(0.010), Some(0.02)),
                roll("r002", Some(0.001), None),
            ],
            vec![],
        );
        let baseline = report(
            Summary {
                de2000_mean: None,
                de2000_p95_mean: None,
                neutrality_mean: None,
            },
            // Different order from the current report, and r002 absent.
            vec![roll("r001", Some(0.001), Some(0.02))],
            vec![],
        );
        let c = compare(&current, &baseline, &tol);
        assert_eq!(
            row(&c, "roll r001 dmin_variance").status,
            Status::Regression
        );
        assert_eq!(row(&c, "roll r001 gamma_variance").status, Status::Ok);
        assert_eq!(
            row(&c, "roll r002 dmin_variance").status,
            Status::NotAvailable
        );
        assert_eq!(
            row(&c, "roll r002 gamma_variance").status,
            Status::NotAvailable
        );
        assert!(c.regressed);
    }

    fn one_frame(de_mean: f32, neutral: Option<f32>) -> EvalReport {
        report(
            Summary {
                de2000_mean: Some(de_mean),
                de2000_p95_mean: Some(de_mean * 2.0),
                neutrality_mean: neutral,
            },
            vec![roll("r001", None, None)],
            vec![frame("r001-03", "r001", de_mean, neutral)],
        )
    }

    #[test]
    fn tracked_case_within_tolerance_is_ok() {
        let mut tol = tolerances();
        tol.case = vec![
            case("r001-03", Some(Tool::Negpy), CaseMetric::De2000Mean, 0.5),
            case("r001-03", Some(Tool::Negpy), CaseMetric::De2000P95, 1.0),
            case("r001-03", Some(Tool::Negpy), CaseMetric::De2000Max, 1.5),
            case("r001-03", None, CaseMetric::Neutrality, 0.2),
        ];
        let c = compare(&one_frame(1.4, Some(0.3)), &one_frame(1.0, Some(0.2)), &tol);
        assert_eq!(row(&c, "case r001-03 negpy de2000_mean").status, Status::Ok);
        assert_eq!(row(&c, "case r001-03 negpy de2000_p95").status, Status::Ok);
        assert_eq!(row(&c, "case r001-03 negpy de2000_max").status, Status::Ok);
        let n = row(&c, "case r001-03 neutrality");
        assert_eq!(n.status, Status::Ok);
        assert_eq!((n.baseline, n.current), (Some(0.2), Some(0.3)));
        assert_eq!(n.case.as_deref(), Some("r001-03"));
        // The summary de2000_mean moved by 0.4 > 0.1, so the gate fails on
        // the summary even though every case is OK.
        assert_eq!(row(&c, "de2000_mean").status, Status::Regression);
        assert!(c.regressed);
    }

    #[test]
    fn tracked_case_beyond_tolerance_regresses_even_when_summary_is_ok() {
        let mut tol = tolerances();
        tol.tolerance.de2000_mean = 10.0;
        tol.tolerance.de2000_p95_mean = 10.0;
        tol.case = vec![case(
            "r001-03",
            Some(Tool::Negpy),
            CaseMetric::De2000Max,
            0.5,
        )];
        let c = compare(&one_frame(1.4, None), &one_frame(1.0, None), &tol);
        assert_eq!(row(&c, "de2000_mean").status, Status::Ok);
        // max goes 3.0 -> 4.2, which is more than 0.5 over.
        assert_eq!(
            row(&c, "case r001-03 negpy de2000_max").status,
            Status::Regression
        );
        assert!(c.regressed);
    }

    #[test]
    fn tracked_case_missing_from_current_is_a_failure() {
        let mut tol = tolerances();
        tol.case = vec![case(
            "r001-03",
            Some(Tool::Negpy),
            CaseMetric::De2000Mean,
            0.5,
        )];
        let baseline = one_frame(1.0, None);
        // Frame gone entirely.
        let c = compare(&summary_only(None), &baseline, &tol);
        let r = row(&c, "case r001-03 negpy de2000_mean");
        assert_eq!(r.status, Status::Missing);
        assert_eq!(r.baseline, Some(1.0));
        assert_eq!(r.current, None);
        assert!(c.regressed);
        // Frame present but no render from that tool.
        let mut current = one_frame(1.0, None);
        current.frames[0].references.clear();
        let c = compare(&current, &baseline, &tol);
        assert_eq!(
            row(&c, "case r001-03 negpy de2000_mean").status,
            Status::Missing
        );
        assert!(c.regressed);
        // Missing only from the baseline cannot be compared yet: n/a.
        let c = compare(&baseline, &summary_only(None), &tol);
        assert_eq!(
            row(&c, "case r001-03 negpy de2000_mean").status,
            Status::NotAvailable
        );
        assert!(!c.regressed);
    }

    #[test]
    fn neutrality_case_on_a_frame_without_a_grey_card_is_not_available() {
        let mut tol = tolerances();
        tol.case = vec![case("r001-03", None, CaseMetric::Neutrality, 0.2)];
        let c = compare(&one_frame(1.0, None), &one_frame(1.0, Some(0.1)), &tol);
        assert_eq!(
            row(&c, "case r001-03 neutrality").status,
            Status::NotAvailable
        );
        assert!(!c.regressed);
    }

    #[test]
    fn table_has_header_rows_and_verdict() {
        let tol = tolerances();
        let ok = compare(&summary_only(Some(1.0)), &summary_only(Some(1.0)), &tol);
        let t = render_table(&ok);
        let lines: Vec<&str> = t.lines().collect();
        assert!(lines[0].starts_with("metric"), "{t}");
        assert!(lines[0].ends_with("status"), "{t}");
        assert!(lines[1].starts_with("de2000_mean"), "{t}");
        assert!(lines[1].contains("+0.0000"), "{t}");
        assert!(lines[1].ends_with("OK"), "{t}");
        assert!(lines[2].ends_with("n/a"), "{t}");
        assert_eq!(*lines.last().unwrap(), "regression gate: OK");
        assert!(!t.contains('\u{2014}') && !t.contains('\u{2013}'));

        let mut tol = tolerances();
        tol.case = vec![case(
            "r001-03",
            Some(Tool::Negpy),
            CaseMetric::De2000Mean,
            0.5,
        )];
        let bad = compare(&summary_only(Some(2.0)), &one_frame(1.0, None), &tol);
        let t = render_table(&bad);
        assert!(t.contains("REGRESSION"), "{t}");
        assert!(t.contains("MISSING"), "{t}");
        assert!(
            t.contains("tracked case r001-03 missing from the current report"),
            "{t}"
        );
        assert!(
            t.trim_end()
                .ends_with("regression gate: FAILED (1 regressed, 1 missing tracked case)"),
            "{t}"
        );
    }
}
