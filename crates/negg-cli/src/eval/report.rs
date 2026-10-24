// SPDX-License-Identifier: Apache-2.0

//! The `eval-results.json` schema and the stdout table.
//!
//! The JSON carries no timestamps or absolute paths so that two CI runs
//! on the same corpus produce byte-identical files and any difference is a
//! real change in a metric.
//!
//! The corpus is local-only (docs/ADR/0005), so a manifest entry whose
//! file is absent on the machine running `negg eval` is skipped rather
//! than failing the run. The report counts those in [`CorpusSummary`] and
//! lists each one in [`EvalReport::skipped`] with a relative path, so the
//! file stays diffable across machines that hold different subsets of the
//! corpus. A frame whose file is present but does not decode, a
//! reference render whose size cannot be matched to the render, and a
//! reference that cannot be registered onto the render (`unaligned`,
//! docs/ADR/0028) are skipped and listed the same way with their own
//! reasons.
//!
//! The fields added for registered references (docs/ADR/0028) are all
//! optional and left out of the JSON when empty, so a report of a corpus
//! without such references is byte-identical to one written before they
//! existed, and older reports still load as baselines: the schema version
//! stays 2.

use std::fmt::Write as _;

use negg_types::{Estimates, PoolRole};
use serde::{Deserialize, Serialize};

use crate::corpus::Tool;
use crate::eval::align::Registration;
use crate::eval::metrics::{DeStats, Neutrality, ToneStats};

/// Schema version written to `eval-results.json`.
///
/// Version 2 (docs/ADR/0019) adds the roll-mode pool fields of
/// [`RollResult`], [`FrameResult::pool`], [`EvalReport::roll_to_roll`],
/// [`EvalReport::three_lights`] and [`Summary::gate2`], and reads
/// `dmin_variance` and `gamma_variance` over the roll's Dmin inliers. A
/// version 1 report is not accepted as a baseline.
pub const REPORT_SCHEMA_VERSION: u32 = 2;

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
    /// One entry per film stock with at least two rolls that have pooled
    /// values, in order of first appearance (Ruling 53).
    #[serde(default)]
    pub roll_to_roll: Vec<RollToRoll>,
    /// One entry per `film_roll` among the evaluated frames, in order of
    /// first appearance (Ruling 53).
    #[serde(default)]
    pub three_lights: Vec<ThreeLights>,
    /// Corpus-wide averages.
    pub summary: Summary,
    /// Every frame and reference render that was skipped because its file
    /// is not present locally, sorted by id then reason. Empty when
    /// nothing was skipped. Absent from reports written before this field
    /// existed, which read back as empty.
    #[serde(default)]
    pub skipped: Vec<Skipped>,
}

/// How many frames the manifest lists, how many were evaluated, and how
/// many of those carry each kind of data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorpusSummary {
    /// Frames listed in the manifest, evaluated or not.
    pub frames: usize,
    /// Frames whose file was present and which were scored. Absent from
    /// reports written before this field existed, which read back as
    /// zero.
    #[serde(default)]
    pub evaluated: usize,
    /// Evaluated frames with at least one evaluated reference render.
    pub with_references: usize,
    /// Evaluated frames with a grey card.
    pub with_grey_card: usize,
    /// Frames skipped because their file is absent or does not decode.
    /// Absent from older reports, which read back as zero.
    #[serde(default)]
    pub skipped_frames: usize,
    /// Reference renders skipped because their file is absent or their
    /// size cannot be matched to the render, counted over evaluated
    /// frames only. Absent from older reports, which read back as zero.
    #[serde(default)]
    pub skipped_references: usize,
}

/// One frame or reference render that `negg eval` did not score: its file
/// is not present in the local corpus, the frame did not decode, or the
/// reference's size could not be matched to the render.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Skipped {
    /// Frame id from the manifest.
    pub id: String,
    /// `file not found: <path>` for a skipped frame,
    /// `reference not found: <path>` for a skipped reference render (paths
    /// relative to the corpus directory as written in the manifest),
    /// `decode failed: <message>` for a frame that does not decode,
    /// `size mismatch: <path> ...` for a reference whose size cannot be
    /// matched to the render, or `unaligned: <path> (...)` for a reference
    /// that could not be registered onto the render.
    pub reason: String,
}

impl Skipped {
    /// The reason prefix for a skipped frame.
    pub const FRAME_PREFIX: &'static str = "file not found: ";
    /// The reason prefix for a skipped reference render.
    pub const REFERENCE_PREFIX: &'static str = "reference not found: ";
    /// The reason prefix for a frame that does not decode.
    pub const DECODE_PREFIX: &'static str = "decode failed: ";
    /// The reason prefix for a reference render whose size cannot be
    /// matched to the render.
    pub const SIZE_PREFIX: &'static str = "size mismatch: ";

    /// A frame whose RAW is absent.
    pub fn frame(id: &str, file: &str) -> Skipped {
        Skipped {
            id: id.to_owned(),
            reason: format!("{}{file}", Skipped::FRAME_PREFIX),
        }
    }

    /// A reference render whose file is absent.
    pub fn reference(id: &str, file: &str) -> Skipped {
        Skipped {
            id: id.to_owned(),
            reason: format!("{}{file}", Skipped::REFERENCE_PREFIX),
        }
    }

    /// A frame whose file is present but does not decode.
    pub fn decode_failed(id: &str, message: &str) -> Skipped {
        Skipped {
            id: id.to_owned(),
            reason: format!("{}{message}", Skipped::DECODE_PREFIX),
        }
    }

    /// The reason prefix for a reference render that could not be
    /// registered onto the render.
    pub const UNALIGNED_PREFIX: &'static str = "unaligned: ";

    /// A reference render that could not be registered onto the render:
    /// its best correlation, when it has one, is below `threshold`.
    pub fn unaligned(id: &str, file: &str, correlation: Option<f32>, threshold: f32) -> Skipped {
        let detail = match correlation {
            Some(c) => format!("best correlation {c:.3} is below {threshold:.2}"),
            None => "no placement covers enough of the render".to_owned(),
        };
        Skipped {
            id: id.to_owned(),
            reason: format!("{}{file} ({detail})", Skipped::UNALIGNED_PREFIX),
        }
    }

    /// A reference render whose size cannot be matched to the render, even
    /// on a shared analysis grid.
    pub fn size_mismatch(
        id: &str,
        file: &str,
        render: (usize, usize),
        reference: (usize, usize),
    ) -> Skipped {
        Skipped {
            id: id.to_owned(),
            reason: format!(
                "{}{file} is {}x{} but the render is {}x{}",
                Skipped::SIZE_PREFIX,
                reference.0,
                reference.1,
                render.0,
                render.1
            ),
        }
    }

    /// Whether this entry is a frame skipped because its file is absent
    /// locally (as opposed to a skipped reference render, or a frame that
    /// did not decode).
    pub fn is_frame(&self) -> bool {
        self.reason.starts_with(Skipped::FRAME_PREFIX)
    }

    /// Whether this entry is a reference render skipped because its file
    /// is absent locally.
    pub fn is_reference(&self) -> bool {
        self.reason.starts_with(Skipped::REFERENCE_PREFIX)
    }
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
    /// Grey-card neutrality on the render, when the frame has a card.
    pub neutrality: Option<Neutrality>,
    /// Everything the engine estimated for the frame (steps 3 to 6 of
    /// `docs/MATHS.md`), as the sidecar records it.
    pub estimates: Option<Estimates>,
    /// CIEDE2000 of the negadoctor reference against the NegPy reference,
    /// when the frame has both and their sizes can be matched (Ruling 32):
    /// the distance Gate 1 compares the engine's against. Absent from
    /// reports written before this field existed, which read back as
    /// `None`.
    #[serde(default)]
    pub reference_distance: Option<DeStats>,
    /// The frame's `film_roll` from the manifest.
    #[serde(default)]
    pub film_roll: Option<String>,
    /// The rig profile the frame was rendered with, as written in the
    /// manifest.
    #[serde(default)]
    pub rig: Option<String>,
    /// The frame's own (unpooled) values and its roles in its roll's
    /// pools, from pass 1 of roll mode (ADR 0017).
    #[serde(default)]
    pub pool: Option<FramePool>,
    /// The references that could not be registered onto the render
    /// (docs/ADR/0028), with the best placement found. They are listed in
    /// [`EvalReport::skipped`] too and count towards no mean. Left out of
    /// the JSON when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unaligned: Vec<UnalignedReference>,
}

/// A reference that could not be registered onto the render.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnalignedReference {
    /// The tool that made the reference.
    pub tool: Tool,
    /// The reference file, relative to the corpus directory.
    pub file: String,
    /// The best placement found, when any candidate covered enough of the
    /// render.
    pub alignment: Option<Alignment>,
}

/// Where a reference was placed on the render (docs/ADR/0028).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Alignment {
    /// Clockwise quarter turn applied to the reference, 0, 90, 180 or 270,
    /// before `flip_horizontal`, as in `Settings.geometry`.
    pub rotate_degrees: u16,
    /// Whether the turned reference is mirrored left to right.
    pub flip_horizontal: bool,
    /// Render pixels per reference pixel.
    pub scale: f32,
    /// Small extra clockwise rotation of the oriented reference about its
    /// centre, in degrees (hand straightening; at most
    /// [`crate::eval::align::MAX_ROTATION_DEGREES`]). Absent in older
    /// reports, read as 0.
    #[serde(default)]
    pub angle_degrees: f32,
    /// Render pixel position `[x, y]` of the centre of the oriented
    /// reference's top-left pixel.
    pub offset: [f32; 2],
    /// Fraction of the render's pixels inside the overlap, the pixels
    /// scored.
    pub overlap: f32,
    /// Normalised cross-correlation of the gradient magnitude of L* over
    /// the overlap: the verdict, aligned at
    /// [`crate::eval::align::MIN_CORRELATION`] or above.
    pub correlation: f32,
    /// Normalised cross-correlation of L* over the overlap: what the
    /// search maximised.
    pub luminance_correlation: f32,
}

impl Alignment {
    /// The record of `registration` on a `w` by `h` render.
    pub fn new(registration: &Registration, w: usize, h: usize) -> Alignment {
        Alignment {
            rotate_degrees: registration.orientation.rotate_degrees,
            flip_horizontal: registration.orientation.flip_horizontal,
            scale: registration.scale as f32,
            angle_degrees: registration.angle_degrees as f32,
            offset: registration.offset.map(|v| v as f32),
            overlap: registration.overlap_fraction(w, h) as f32,
            correlation: registration.correlation as f32,
            luminance_correlation: registration.luminance_correlation as f32,
        }
    }
}

/// One frame's record in its roll's pools (R-pool, R-cast-pool): what it
/// estimated on its own and whether it fed each pool.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FramePool {
    /// The frame's own Dmin (its Fixed value when the user fixed it).
    pub dmin: [f32; 3],
    /// The frame's own gamma, `max(Dmax - Dmin_pooled, 0.05)` (its Fixed
    /// value when the user fixed it).
    pub gamma: [f32; 3],
    /// Its role in the Dmin pool. The Dmin inliers are the frames Gate 2
    /// measures (Ruling 48).
    pub dmin_role: PoolRole,
    /// Its role in the gamma pool.
    pub gamma_role: PoolRole,
    /// Its role in the cast pool.
    pub cast_role: PoolRole,
}

/// CIEDE2000 against one reference render.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReferenceResult {
    /// The tool that made the reference.
    pub tool: Tool,
    /// The colour-difference statistics.
    pub de2000: DeStats,
    /// Where the reference was placed on the render, when it was
    /// registered (docs/ADR/0028); the statistics are then over the
    /// overlap only. Left out of the JSON for a reference compared pixel
    /// by pixel or on the shared analysis grid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alignment: Option<Alignment>,
    /// Tone and colour descriptors of the render and the registered
    /// reference over the overlap. Present exactly when `alignment` is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<ToneStats>,
}

/// Per-roll consistency of the estimates. The statistics are taken over
/// the roll's Dmin inliers (Ruling 48) using each frame's own, unpooled
/// values from pass 1 (Ruling 53), and are `None` with fewer than two
/// inliers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollResult {
    /// Roll id.
    pub roll: String,
    /// Number of evaluated frames in the roll.
    pub frames: usize,
    /// Population variance of the per-frame Dmin per channel over the
    /// inliers (`dmin_std` squared).
    pub dmin_variance: Option<[f32; 3]>,
    /// Population variance of the per-frame gamma per channel over the
    /// inliers (`gamma_std` squared).
    pub gamma_variance: Option<[f32; 3]>,
    /// Number of evaluated frames that are Dmin inliers.
    #[serde(default)]
    pub inliers: usize,
    /// Ids of the evaluated frames Rejected from the Dmin pool, in
    /// manifest order.
    #[serde(default)]
    pub rejected: Vec<String>,
    /// Population standard deviation (log-D) of the per-frame Dmin per
    /// channel over the inliers: the Gate 2 Dmin term (Ruling 53).
    #[serde(default)]
    pub dmin_std: Option<[f32; 3]>,
    /// Population standard deviation of the per-frame gamma per channel
    /// over the inliers.
    #[serde(default)]
    pub gamma_std: Option<[f32; 3]>,
    /// The roll's pooled Dmin, when it has one.
    #[serde(default)]
    pub pooled_dmin: Option<[f32; 3]>,
    /// The roll's pooled gamma ratios `[R/G, B/G]`, when it has a pooled
    /// gamma.
    #[serde(default)]
    pub pooled_gamma_ratio: Option<[f32; 2]>,
}

/// Consistency across the rolls of one film stock (Ruling 53): the
/// population standard deviation across rolls of the pooled values.
/// Reported and tolerance-gated, not a Gate 2 term.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RollToRoll {
    /// The film stock.
    pub stock: String,
    /// Number of rolls of the stock with both a pooled Dmin and a pooled
    /// gamma; at least two.
    pub rolls: usize,
    /// Standard deviation across those rolls of the pooled Dmin, per
    /// channel (log-D).
    pub dmin_std: [f32; 3],
    /// Standard deviation across those rolls of the pooled gamma ratios
    /// `[R/G, B/G]`.
    pub gamma_ratio_std: [f32; 2],
}

/// Grey-card neutrality of one light source within a film roll.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LightResult {
    /// The backlight.
    pub light_source: String,
    /// Number of evaluated grey-card frames under this light.
    pub frames: usize,
    /// Mean [`Neutrality::de`] of those frames.
    pub neutrality_de_mean: f32,
}

/// The Gate 2 three-lights test for one film roll (Ruling 53).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreeLights {
    /// The `film_roll` from the manifest.
    pub film_roll: String,
    /// One entry per light source among the film roll's evaluated
    /// grey-card frames, in order of first appearance.
    pub lights: Vec<LightResult>,
    /// Whether the film roll qualifies: its evaluated grey-card frames
    /// span at least [`GATE2_MIN_LIGHTS`] light sources and every one of
    /// them was rendered with a rig profile.
    pub qualifies: bool,
    /// The largest per-light mean; `None` when there are no lights. Only
    /// qualifying film rolls count towards Gate 2.
    pub max_de: Option<f32>,
}

/// The largest channel of a per-channel variance: what the table prints
/// and the regression gate compares.
pub fn max_channel(v: Option<[f32; 3]>) -> Option<f32> {
    v.map(|v| v[0].max(v[1]).max(v[2]))
}

/// Corpus-wide averages. Each is `None` when nothing contributed to it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    /// Mean of the per-reference CIEDE2000 means.
    pub de2000_mean: Option<f32>,
    /// Mean of the per-reference CIEDE2000 95th percentiles.
    pub de2000_p95_mean: Option<f32>,
    /// Mean over frames with a neutrality score of `(|a*| + |b*|) / 2`.
    pub neutrality_mean: Option<f32>,
    /// [`Summary::de2000_mean`] split by reference tool (Ruling 32).
    /// Absent from older reports, which read back as all `None`.
    #[serde(default)]
    pub de2000_mean_by_tool: ByTool,
    /// The plan's Gate 1 corpus criteria (Ruling 32). Absent from older
    /// reports, which read back as all `None`.
    #[serde(default)]
    pub gate1: Gate1,
    /// The plan's Gate 2 criteria (Ruling 53).
    #[serde(default)]
    pub gate2: Gate2,
    /// Means of the tone and colour descriptors over the registered
    /// references (docs/ADR/0028); left out of the JSON when there are
    /// none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<ToneSummary>,
}

/// Means over registered references of how the render's tone and colour
/// differ from the reference's (render minus reference), signed to show
/// the direction and absolute to gate on.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ToneMeans {
    /// Number of registered references averaged.
    pub references: usize,
    /// Mean difference of L* at the 1st percentile.
    pub l_p1_diff: f32,
    /// Mean difference of L* at the 50th percentile.
    pub l_p50_diff: f32,
    /// Mean difference of L* at the 99th percentile.
    pub l_p99_diff: f32,
    /// Mean difference of the mean chroma C*ab.
    pub chroma_diff: f32,
    /// Mean of [`ToneStats::a_diff`].
    pub a_diff: f32,
    /// Mean of [`ToneStats::b_diff`].
    pub b_diff: f32,
    /// Mean over references of the mean absolute difference of the three
    /// L* percentiles: how far the tone curve is off. Gated.
    pub l_abs_diff: f32,
    /// Mean absolute difference of the mean chroma. Gated.
    pub chroma_abs_diff: f32,
    /// Mean of `hypot(a_diff, b_diff)`: the size of the cast. Gated.
    pub cast_ab: f32,
}

impl ToneMeans {
    /// The means over `tones`, or `None` when there are none.
    pub fn over(tones: &[ToneStats]) -> Option<ToneMeans> {
        if tones.is_empty() {
            return None;
        }
        let n = tones.len() as f64;
        let mean = |f: &dyn Fn(&ToneStats) -> f32| {
            (tones.iter().map(|t| f64::from(f(t))).sum::<f64>() / n) as f32
        };
        Some(ToneMeans {
            references: tones.len(),
            l_p1_diff: mean(&|t| t.render.l_p1 - t.reference.l_p1),
            l_p50_diff: mean(&|t| t.render.l_p50 - t.reference.l_p50),
            l_p99_diff: mean(&|t| t.render.l_p99 - t.reference.l_p99),
            chroma_diff: mean(&|t| t.render.chroma_mean - t.reference.chroma_mean),
            a_diff: mean(&|t| t.a_diff),
            b_diff: mean(&|t| t.b_diff),
            l_abs_diff: mean(&|t| {
                ((t.render.l_p1 - t.reference.l_p1).abs()
                    + (t.render.l_p50 - t.reference.l_p50).abs()
                    + (t.render.l_p99 - t.reference.l_p99).abs())
                    / 3.0
            }),
            chroma_abs_diff: mean(&|t| (t.render.chroma_mean - t.reference.chroma_mean).abs()),
            cast_ab: mean(&|t| t.a_diff.hypot(t.b_diff)),
        })
    }
}

/// [`ToneMeans`] over every registered reference and per tool.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ToneSummary {
    /// Over every registered reference.
    pub all: ToneMeans,
    /// Over the registered NegPy references.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub negpy: Option<ToneMeans>,
    /// Over the registered negadoctor references.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub negadoctor: Option<ToneMeans>,
    /// Over the Negative Lab Pro references.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nlp: Option<ToneMeans>,
}

/// A mean per reference tool; `None` for a tool with no scored reference.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ByTool {
    /// Over the NegPy references.
    pub negpy: Option<f32>,
    /// Over the negadoctor references.
    pub negadoctor: Option<f32>,
    /// Over the Negative Lab Pro references (docs/ADR/0028). Left out of
    /// the JSON when there are none, so older reports are unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nlp: Option<f32>,
}

/// The neutrality bound of Gate 1: "grey-card neutrality error is under
/// dE 4" (`IMPLEMENTATION_PLAN.md`).
pub const GATE1_NEUTRALITY_DE: f32 = 4.0;

/// Gate 1 of the plan: "on the corpus, default renders have lower mean
/// CIEDE2000 to the NegPy reference than the negadoctor reference does,
/// and grey-card neutrality error is under dE 4". Each field is `None`
/// when the corpus has no data for it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Gate1 {
    /// Mean CIEDE2000 of the engine's render to the NegPy reference, over
    /// the frames that have a [`FrameResult::reference_distance`] (both
    /// references), so it is taken over the same frames as
    /// [`Gate1::negadoctor_vs_negpy`].
    pub engine_vs_negpy: Option<f32>,
    /// Mean of [`FrameResult::reference_distance`]: the negadoctor
    /// reference's CIEDE2000 to the NegPy reference.
    pub negadoctor_vs_negpy: Option<f32>,
    /// Mean of [`Neutrality::de`] over frames with a grey card.
    pub neutrality_de_mean: Option<f32>,
    /// Whether Gate 1 passes, present when both distances are: the engine
    /// is closer to NegPy than negadoctor is and, when there is a grey
    /// card, `neutrality_de_mean` is under [`GATE1_NEUTRALITY_DE`].
    pub pass: Option<bool>,
}

impl Gate1 {
    /// Fills in [`Gate1::pass`] from the other three fields.
    pub fn new(
        engine_vs_negpy: Option<f32>,
        negadoctor_vs_negpy: Option<f32>,
        neutrality_de_mean: Option<f32>,
    ) -> Gate1 {
        let pass = match (engine_vs_negpy, negadoctor_vs_negpy) {
            (Some(engine), Some(negadoctor)) => Some(
                engine < negadoctor && neutrality_de_mean.is_none_or(|d| d < GATE1_NEUTRALITY_DE),
            ),
            _ => None,
        };
        Gate1 {
            engine_vs_negpy,
            negadoctor_vs_negpy,
            neutrality_de_mean,
            pass,
        }
    }
}

/// The Dmin bound of Gate 2: "roll-level Dmin variance under 0.02 log-D
/// across inlier frames", read as a standard deviation (Ruling 53).
pub const GATE2_DMIN_STD: f32 = 0.02;

/// The neutrality bound of Gate 2: "grey-card neutrality under dE 2 on the
/// same roll scanned under three different light sources with a rig
/// profile".
pub const GATE2_NEUTRALITY_DE: f32 = 2.0;

/// The number of distinct light sources a film roll needs to qualify for
/// the three-lights test.
pub const GATE2_MIN_LIGHTS: usize = 3;

/// Gate 2 of the plan (Ruling 53). Each field is `None` when the corpus
/// has no data for it; a `None` pass is not a pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Gate2 {
    /// The largest channel of [`RollResult::dmin_std`] over the rolls
    /// with at least two inliers.
    pub dmin_std_max: Option<f32>,
    /// The largest [`ThreeLights::max_de`] over the qualifying film rolls.
    pub three_lights_max_de: Option<f32>,
    /// `dmin_std_max < GATE2_DMIN_STD && three_lights_max_de <
    /// GATE2_NEUTRALITY_DE`, present only when both are.
    pub pass: Option<bool>,
}

impl Gate2 {
    /// Fills in [`Gate2::pass`] from the other two fields.
    pub fn new(dmin_std_max: Option<f32>, three_lights_max_de: Option<f32>) -> Gate2 {
        let pass = match (dmin_std_max, three_lights_max_de) {
            (Some(d), Some(de)) => Some(d < GATE2_DMIN_STD && de < GATE2_NEUTRALITY_DE),
            _ => None,
        };
        Gate2 {
            dmin_std_max,
            three_lights_max_de,
            pass,
        }
    }
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
/// row per roll (standard deviations and variances as the largest
/// channel), then, when present, one row per stock for the roll-to-roll
/// metric, one row per film roll and light for the three-lights test and
/// one row per registered reference (placement and tone, render /
/// reference), then the summary line, the Gate 1 line (with the per-tool
/// means), the Gate 2 line, the tone line when any reference was
/// registered, then the skipped line (see [`skipped_line`]) followed by one
/// indented `<id>: <reason>` line per skipped entry. When the manifest lists no frames at all the tables are
/// replaced by the single line `No frames in corpus`; when it lists frames
/// but every one was skipped the table headers are printed with no rows.
/// Column widths grow to fit their content; numbers are right-aligned.
pub fn render_table(report: &EvalReport) -> String {
    let mut out = String::new();
    if report.corpus.frames == 0 {
        out.push_str("No frames in corpus\n");
    } else {
        write_frame_table(&mut out, report);
        out.push('\n');
        write_roll_table(&mut out, report);
        out.push('\n');
        if !report.roll_to_roll.is_empty() {
            write_roll_to_roll_table(&mut out, report);
            out.push('\n');
        }
        if !report.three_lights.is_empty() {
            write_three_lights_table(&mut out, report);
            out.push('\n');
        }
        if report
            .frames
            .iter()
            .any(|f| f.references.iter().any(|r| r.alignment.is_some()))
        {
            write_alignment_table(&mut out, report);
            out.push('\n');
        }
    }
    let s = &report.summary;
    let c = &report.corpus;
    let _ = writeln!(
        out,
        "summary: dE mean {}  dE p95 mean {}  neutrality mean {}  ({} frames, {} evaluated, {} with references, {} with grey card)",
        opt(s.de2000_mean),
        opt(s.de2000_p95_mean),
        opt(s.neutrality_mean),
        c.frames,
        c.evaluated,
        c.with_references,
        c.with_grey_card
    );
    let g = &s.gate1;
    let nlp = s
        .de2000_mean_by_tool
        .nlp
        .map_or_else(String::new, |v| format!(", nlp {v:.4}"));
    let _ = writeln!(
        out,
        "gate 1: engine vs negpy {}  negadoctor vs negpy {}  neutrality dE mean {}  pass {}  \
         (dE mean by tool: negpy {}, negadoctor {}{nlp})",
        opt(g.engine_vs_negpy),
        opt(g.negadoctor_vs_negpy),
        opt(g.neutrality_de_mean),
        g.pass.map_or("-", |p| if p { "yes" } else { "no" }),
        opt(s.de2000_mean_by_tool.negpy),
        opt(s.de2000_mean_by_tool.negadoctor)
    );
    let g = &s.gate2;
    let _ = writeln!(
        out,
        "gate 2: dmin std max {}  three lights max dE {}  pass {}",
        opt(g.dmin_std_max),
        opt(g.three_lights_max_de),
        g.pass.map_or("-", |p| if p { "yes" } else { "no" })
    );
    if let Some(t) = &s.tone {
        let m = &t.all;
        let _ = writeln!(
            out,
            "tone (render minus reference, {} registered): L* p1 {:+.2}  p50 {:+.2}  p99 {:+.2}  \
             C* {:+.2}  a* {:+.2}  b* {:+.2}  (|L*| {:.2}, |C*| {:.2}, cast {:.2})",
            m.references,
            m.l_p1_diff,
            m.l_p50_diff,
            m.l_p99_diff,
            m.chroma_diff,
            m.a_diff,
            m.b_diff,
            m.l_abs_diff,
            m.chroma_abs_diff,
            m.cast_ab
        );
    }
    out.push_str(&skipped_line(report));
    out.push('\n');
    for skipped in &report.skipped {
        let _ = writeln!(out, "  {}: {}", skipped.id, skipped.reason);
    }
    out
}

/// The one-line count of skipped entries, without a trailing newline:
/// `Skipped <N> frame(s) and <M> reference render(s)`. Printed even when
/// both counts are zero so the output shape is stable; the per-entry
/// lines that follow carry the reasons.
pub fn skipped_line(report: &EvalReport) -> String {
    format!(
        "Skipped {} frame(s) and {} reference render(s)",
        report.corpus.skipped_frames, report.corpus.skipped_references
    )
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

/// One row per registered reference: where it was placed and how the
/// render's tone and colour differ from it (render / reference).
fn write_alignment_table(out: &mut String, report: &EvalReport) {
    let mut rows = vec![header(&[
        "id", "tool", "turn", "flip", "scale", "angle", "offset", "overlap", "corr", "L* p1",
        "L* p50", "L* p99", "C*", "da*", "db*",
    ])];
    let pair = |a: f32, b: f32| format!("{a:.1}/{b:.1}");
    for frame in &report.frames {
        for reference in &frame.references {
            let (Some(a), Some(t)) = (reference.alignment, reference.tone) else {
                continue;
            };
            rows.push(vec![
                frame.id.clone(),
                reference.tool.name().to_owned(),
                a.rotate_degrees.to_string(),
                if a.flip_horizontal { "yes" } else { "no" }.to_owned(),
                format!("{:.4}", a.scale),
                format!("{:+.2}", a.angle_degrees),
                format!("{:.1},{:.1}", a.offset[0], a.offset[1]),
                format!("{:.3}", a.overlap),
                format!("{:.3}", a.correlation),
                pair(t.render.l_p1, t.reference.l_p1),
                pair(t.render.l_p50, t.reference.l_p50),
                pair(t.render.l_p99, t.reference.l_p99),
                pair(t.render.chroma_mean, t.reference.chroma_mean),
                format!("{:+.2}", t.a_diff),
                format!("{:+.2}", t.b_diff),
            ]);
        }
    }
    write_aligned(
        out,
        &rows,
        &[
            false, false, true, false, true, true, true, true, true, true, true, true, true, true,
            true,
        ],
    );
}

fn header(cells: &[&str]) -> Vec<String> {
    cells.iter().map(|s| (*s).to_owned()).collect()
}

fn write_roll_table(out: &mut String, report: &EvalReport) {
    let mut rows = vec![header(&[
        "roll",
        "frames",
        "inliers",
        "rejected",
        "dmin std (max)",
        "gamma std (max)",
        "dmin var (max)",
        "gamma var (max)",
    ])];
    for roll in &report.rolls {
        rows.push(vec![
            roll.roll.clone(),
            roll.frames.to_string(),
            roll.inliers.to_string(),
            roll.rejected.len().to_string(),
            opt(max_channel(roll.dmin_std)),
            opt(max_channel(roll.gamma_std)),
            opt(max_channel(roll.dmin_variance)),
            opt(max_channel(roll.gamma_variance)),
        ]);
    }
    write_aligned(
        out,
        &rows,
        &[false, true, true, true, true, true, true, true],
    );
}

fn write_roll_to_roll_table(out: &mut String, report: &EvalReport) {
    let mut rows = vec![header(&[
        "stock",
        "rolls",
        "dmin std (max)",
        "gamma ratio std (max)",
    ])];
    for r in &report.roll_to_roll {
        rows.push(vec![
            r.stock.clone(),
            r.rolls.to_string(),
            format!("{:.4}", r.dmin_std[0].max(r.dmin_std[1]).max(r.dmin_std[2])),
            format!("{:.4}", r.gamma_ratio_std[0].max(r.gamma_ratio_std[1])),
        ]);
    }
    write_aligned(out, &rows, &[false, true, true, true]);
}

fn write_three_lights_table(out: &mut String, report: &EvalReport) {
    let mut rows = vec![header(&[
        "film_roll",
        "light_source",
        "frames",
        "dE mean",
        "qualifies",
    ])];
    for t in &report.three_lights {
        let qualifies = if t.qualifies { "yes" } else { "no" };
        if t.lights.is_empty() {
            rows.push(vec![
                t.film_roll.clone(),
                "-".to_owned(),
                "0".to_owned(),
                "-".to_owned(),
                qualifies.to_owned(),
            ]);
        }
        for light in &t.lights {
            rows.push(vec![
                t.film_roll.clone(),
                light.light_source.clone(),
                light.frames.to_string(),
                format!("{:.4}", light.neutrality_de_mean),
                qualifies.to_owned(),
            ]);
        }
    }
    write_aligned(out, &rows, &[false, false, true, true, false]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use negg_types::{CastCorrection, DminSource, GammaSource};

    fn empty_report() -> EvalReport {
        EvalReport {
            schema_version: REPORT_SCHEMA_VERSION,
            corpus: CorpusSummary {
                frames: 0,
                evaluated: 0,
                with_references: 0,
                with_grey_card: 0,
                skipped_frames: 0,
                skipped_references: 0,
            },
            frames: vec![],
            rolls: vec![],
            roll_to_roll: vec![],
            three_lights: vec![],
            summary: Summary {
                de2000_mean: None,
                de2000_p95_mean: None,
                neutrality_mean: None,
                de2000_mean_by_tool: ByTool::default(),
                gate1: Gate1::default(),
                gate2: Gate2::default(),
                tone: None,
            },
            skipped: vec![],
        }
    }

    /// Two frames in the manifest, both skipped.
    fn all_skipped_report() -> EvalReport {
        let mut r = empty_report();
        r.corpus.frames = 2;
        r.corpus.skipped_frames = 2;
        r.skipped = vec![
            Skipped::frame("r001-01", "frames/r001/r001-01.dng"),
            Skipped::frame("r001-02", "frames/r001/r001-02.dng"),
        ];
        r
    }

    fn one_frame_report() -> EvalReport {
        EvalReport {
            schema_version: REPORT_SCHEMA_VERSION,
            corpus: CorpusSummary {
                frames: 1,
                evaluated: 1,
                with_references: 1,
                with_grey_card: 1,
                skipped_frames: 0,
                skipped_references: 0,
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
                    alignment: None,
                    tone: None,
                }],
                neutrality: Some(Neutrality {
                    a_mean_abs: 0.5,
                    b_mean_abs: 0.25,
                    de: 0.3,
                }),
                estimates: Some(Estimates {
                    dmin: [0.2, 0.5, 0.7],
                    dmin_source: DminSource::Border,
                    dmax: [1.9, 2.2, 2.4],
                    gamma: [1.7, 1.7, 1.7],
                    gamma_source: GammaSource::Range,
                    neutral_offset: [0.0; 3],
                    anchor: -0.4,
                    x0: 1.2,
                    cast: CastCorrection::IDENTITY,
                    cast_source: negg_types::CastSource::Identity,
                }),
                reference_distance: None,
                film_roll: None,
                rig: None,
                pool: None,
                unaligned: vec![],
            }],
            rolls: vec![RollResult {
                roll: "r001".into(),
                frames: 1,
                dmin_variance: None,
                gamma_variance: None,
                inliers: 1,
                rejected: vec![],
                dmin_std: None,
                gamma_std: None,
                pooled_dmin: Some([0.2, 0.5, 0.7]),
                pooled_gamma_ratio: Some([1.0, 1.0]),
            }],
            roll_to_roll: vec![],
            three_lights: vec![],
            summary: Summary {
                de2000_mean: Some(0.0),
                de2000_p95_mean: Some(0.0),
                neutrality_mean: Some(0.375),
                de2000_mean_by_tool: ByTool {
                    negpy: Some(0.0),
                    negadoctor: None,
                    nlp: None,
                },
                gate1: Gate1::new(None, None, Some(0.3)),
                gate2: Gate2::default(),
                tone: None,
            },
            skipped: vec![],
        }
    }

    #[test]
    fn empty_json_matches_schema() {
        let text = to_json(&empty_report()).unwrap();
        assert!(text.ends_with('\n'));
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["schema_version"], 2);
        assert_eq!(v["corpus"]["frames"], 0);
        assert_eq!(v["corpus"]["evaluated"], 0);
        assert_eq!(v["corpus"]["skipped_frames"], 0);
        assert_eq!(v["corpus"]["skipped_references"], 0);
        assert_eq!(v["frames"], serde_json::json!([]));
        assert_eq!(v["rolls"], serde_json::json!([]));
        assert!(v["summary"]["de2000_mean"].is_null());
        assert!(v["summary"]["de2000_p95_mean"].is_null());
        assert!(v["summary"]["neutrality_mean"].is_null());
        assert!(v["summary"]["de2000_mean_by_tool"]["negpy"].is_null());
        assert!(v["summary"]["de2000_mean_by_tool"]["negadoctor"].is_null());
        for key in [
            "engine_vs_negpy",
            "negadoctor_vs_negpy",
            "neutrality_de_mean",
            "pass",
        ] {
            assert!(v["summary"]["gate1"][key].is_null(), "{key}");
        }
        for key in ["dmin_std_max", "three_lights_max_de", "pass"] {
            assert!(v["summary"]["gate2"][key].is_null(), "{key}");
        }
        assert_eq!(v["roll_to_roll"], serde_json::json!([]));
        assert_eq!(v["three_lights"], serde_json::json!([]));
        assert_eq!(v["skipped"], serde_json::json!([]));
    }

    #[test]
    fn gate1_passes_only_when_closer_to_negpy_and_neutral() {
        assert_eq!(Gate1::new(None, Some(2.0), Some(1.0)).pass, None);
        assert_eq!(Gate1::new(Some(1.0), None, None).pass, None);
        assert_eq!(Gate1::new(Some(1.0), Some(2.0), None).pass, Some(true));
        assert_eq!(Gate1::new(Some(1.0), Some(2.0), Some(3.9)).pass, Some(true));
        assert_eq!(
            Gate1::new(Some(1.0), Some(2.0), Some(4.0)).pass,
            Some(false)
        );
        assert_eq!(
            Gate1::new(Some(2.0), Some(2.0), Some(1.0)).pass,
            Some(false)
        );
    }

    #[test]
    fn skipped_entries_are_listed_in_the_json() {
        let text = to_json(&all_skipped_report()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["corpus"]["frames"], 2);
        assert_eq!(v["corpus"]["evaluated"], 0);
        assert_eq!(v["corpus"]["skipped_frames"], 2);
        assert_eq!(v["skipped"][0]["id"], "r001-01");
        assert_eq!(
            v["skipped"][0]["reason"],
            "file not found: frames/r001/r001-01.dng"
        );
        assert_eq!(v["skipped"][1]["id"], "r001-02");
        let back: EvalReport = serde_json::from_str(&text).unwrap();
        assert_eq!(back.skipped, all_skipped_report().skipped);
    }

    #[test]
    fn reports_without_the_skipped_fields_still_load() {
        // A results file written before docs/ADR/0005 lacks `evaluated`,
        // `skipped_frames`, `skipped_references` and `skipped`.
        let old = r#"{
          "schema_version": 1,
          "corpus": { "frames": 0, "with_references": 0, "with_grey_card": 0 },
          "frames": [],
          "rolls": [],
          "summary": { "de2000_mean": null, "de2000_p95_mean": null, "neutrality_mean": null }
        }"#;
        let r: EvalReport = serde_json::from_str(old).unwrap();
        assert_eq!(r.corpus, empty_report().corpus);
        assert!(r.skipped.is_empty());
        // Nor the Ruling 32 fields: per-tool means, gate1, the per-frame
        // reference distance and neutrality.de.
        assert_eq!(r.summary, empty_report().summary);
        let old_frame = r#"{
          "id": "a", "roll": "r", "stock": "s", "camera": "c", "light_source": "l",
          "references": [],
          "neutrality": { "a_mean_abs": 1.0, "b_mean_abs": 2.0 },
          "estimates": null
        }"#;
        let f: FrameResult = serde_json::from_str(old_frame).unwrap();
        assert!(f.reference_distance.is_none());
        assert_eq!(f.neutrality.unwrap().de, 0.0);
    }

    #[test]
    fn skipped_constructors_and_prefixes_agree() {
        let f = Skipped::frame("r001-01", "frames/r001/r001-01.dng");
        assert!(f.is_frame() && !f.is_reference());
        let r = Skipped::reference("r001-01", "reference/r001-01.negpy.tif");
        assert!(r.is_reference() && !r.is_frame());
        assert_eq!(r.reason, "reference not found: reference/r001-01.negpy.tif");
        // Sorting is by id, then reason.
        let mut v = vec![r.clone(), Skipped::frame("r001-02", "b"), f.clone()];
        v.sort();
        assert_eq!(v, vec![f, r, Skipped::frame("r001-02", "b")]);
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
        assert!((f["neutrality"]["de"].as_f64().unwrap() - 0.3).abs() < 1e-6);
        assert!(f["reference_distance"].is_null());
        assert_eq!(v["summary"]["de2000_mean_by_tool"]["negpy"], 0.0);
        assert!(v["summary"]["gate1"]["pass"].is_null());
        assert_eq!(f["estimates"]["dmin_source"], "border");
        assert_eq!(f["estimates"]["gamma_source"], "range");
        assert_eq!(f["estimates"]["cast"]["slope_r"], 1.0);
        assert!(v["rolls"][0]["dmin_variance"].is_null());
        assert!(v["rolls"][0]["gamma_variance"].is_null());
        assert_eq!(v["rolls"][0]["frames"], 1);
        assert_eq!(v["summary"]["de2000_mean"], 0.0);
        // Round trip through the typed structs too.
        let back: EvalReport = serde_json::from_str(&text).unwrap();
        assert_eq!(back.frames[0].references[0].tool, Tool::Negpy);
    }

    #[test]
    fn per_channel_variances_serialise_as_arrays_and_print_their_max() {
        let mut r = one_frame_report();
        r.rolls[0].frames = 2;
        r.rolls[0].dmin_variance = Some([0.001, 0.003, 0.002]);
        r.rolls[0].gamma_variance = Some([0.01, 0.0, 0.0]);
        let text = to_json(&r).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            v["rolls"][0]["dmin_variance"],
            serde_json::json!([0.001, 0.003, 0.002])
        );
        let back: EvalReport = serde_json::from_str(&text).unwrap();
        assert_eq!(back.rolls[0].gamma_variance, Some([0.01, 0.0, 0.0]));
        assert_eq!(max_channel(r.rolls[0].dmin_variance), Some(0.003));
        assert_eq!(max_channel(None), None);
        let t = render_table(&r);
        assert!(
            t.lines()
                .any(|l| l.starts_with("r001 ") && l.contains("0.0030") && l.ends_with("0.0100")),
            "{t}"
        );
    }

    #[test]
    fn skipped_reasons_for_decode_and_size() {
        let d = Skipped::decode_failed("r001-01", "RAW error: bad");
        assert_eq!(d.reason, "decode failed: RAW error: bad");
        assert!(!d.is_frame() && !d.is_reference());
        let s = Skipped::size_mismatch("r001-01", "reference/x.tif", (100, 50), (30, 30));
        assert_eq!(
            s.reason,
            "size mismatch: reference/x.tif is 30x30 but the render is 100x50"
        );
        assert!(!s.is_frame() && !s.is_reference());
    }

    #[test]
    fn empty_table_says_so() {
        let t = render_table(&empty_report());
        assert!(t.starts_with("No frames in corpus\n"), "{t}");
        assert!(
            t.contains("summary: dE mean -  dE p95 mean -  neutrality mean -"),
            "{t}"
        );
        assert!(
            t.ends_with("Skipped 0 frame(s) and 0 reference render(s)\n"),
            "{t}"
        );
    }

    #[test]
    fn all_skipped_table_has_headers_and_skipped_lines() {
        let t = render_table(&all_skipped_report());
        let lines: Vec<&str> = t.lines().collect();
        assert!(!t.contains("No frames in corpus"), "{t}");
        assert!(lines[0].starts_with("id"), "{t}");
        assert_eq!(lines[1], "", "{t}");
        assert!(lines[2].starts_with("roll"), "{t}");
        assert_eq!(lines[3], "", "{t}");
        assert!(lines[4].starts_with("summary:"), "{t}");
        assert!(lines[4].contains("(2 frames, 0 evaluated,"), "{t}");
        assert!(lines[5].starts_with("gate 1: engine vs negpy -"), "{t}");
        assert_eq!(
            lines[6],
            "gate 2: dmin std max -  three lights max dE -  pass -"
        );
        assert_eq!(lines[7], "Skipped 2 frame(s) and 0 reference render(s)");
        assert_eq!(
            lines[8],
            "  r001-01: file not found: frames/r001/r001-01.dng"
        );
        assert_eq!(
            lines[9],
            "  r001-02: file not found: frames/r001/r001-02.dng"
        );
        assert_eq!(lines.len(), 10);
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
        let summary = lines[lines.len() - 4];
        assert!(summary.starts_with("summary:"), "{t}");
        let gate = lines[lines.len() - 3];
        assert_eq!(
            gate,
            "gate 1: engine vs negpy -  negadoctor vs negpy -  neutrality dE mean 0.3000  pass -  \
             (dE mean by tool: negpy 0.0000, negadoctor -)",
            "{t}"
        );
        assert!(
            summary.contains("(1 frames, 1 evaluated, 1 with references, 1 with grey card)"),
            "{t}"
        );
        assert_eq!(
            *lines.last().unwrap(),
            "Skipped 0 frame(s) and 0 reference render(s)"
        );
        assert!(!t.contains('\u{2014}') && !t.contains('\u{2013}'));
    }

    #[test]
    fn gate2_passes_fails_and_is_null() {
        assert_eq!(Gate2::new(None, None).pass, None);
        assert_eq!(Gate2::new(Some(0.01), None).pass, None);
        assert_eq!(Gate2::new(None, Some(1.0)).pass, None);
        assert_eq!(Gate2::new(Some(0.01), Some(1.0)).pass, Some(true));
        assert_eq!(Gate2::new(Some(0.02), Some(1.0)).pass, Some(false));
        assert_eq!(Gate2::new(Some(0.01), Some(2.0)).pass, Some(false));
    }

    #[test]
    fn gate2_tables_and_line_are_printed() {
        let mut r = one_frame_report();
        r.rolls[0].inliers = 3;
        r.rolls[0].rejected = vec!["r001-04".into()];
        r.rolls[0].dmin_std = Some([0.01, 0.015, 0.005]);
        r.roll_to_roll = vec![RollToRoll {
            stock: "Kodak Portra 400".into(),
            rolls: 2,
            dmin_std: [0.01, 0.03, 0.02],
            gamma_ratio_std: [0.002, 0.004],
        }];
        r.three_lights = vec![ThreeLights {
            film_roll: "f001".into(),
            lights: vec![LightResult {
                light_source: "Cinestill CS-Lite".into(),
                frames: 2,
                neutrality_de_mean: 1.25,
            }],
            qualifies: false,
            max_de: Some(1.25),
        }];
        r.summary.gate2 = Gate2::new(Some(0.015), Some(1.5));
        let t = render_table(&r);
        assert!(
            t.lines()
                .any(|l| l.starts_with("r001 ") && l.contains(" 3 ") && l.contains("0.0150")),
            "{t}"
        );
        assert!(
            t.lines()
                .any(|l| l.starts_with("Kodak Portra 400") && l.contains("0.0300")),
            "{t}"
        );
        assert!(
            t.lines()
                .any(|l| l.starts_with("f001") && l.contains("1.2500") && l.ends_with("no")),
            "{t}"
        );
        assert!(
            t.contains("gate 2: dmin std max 0.0150  three lights max dE 1.5000  pass yes"),
            "{t}"
        );
        let back: EvalReport = serde_json::from_str(&to_json(&r).unwrap()).unwrap();
        assert_eq!(back.roll_to_roll, r.roll_to_roll);
        assert_eq!(back.three_lights, r.three_lights);
        assert_eq!(back.summary.gate2, r.summary.gate2);
        assert_eq!(back.rolls[0].rejected, vec!["r001-04".to_owned()]);
    }

    fn tone(l: [f32; 3], chroma: f32, a: f32, b: f32) -> crate::eval::metrics::ToneDescriptors {
        crate::eval::metrics::ToneDescriptors {
            l_p1: l[0],
            l_p50: l[1],
            l_p99: l[2],
            chroma_mean: chroma,
            a_mean: a,
            b_mean: b,
        }
    }

    fn registered_report() -> EvalReport {
        let mut r = one_frame_report();
        let stats = ToneStats {
            render: tone([5.0, 50.0, 95.0], 20.0, 1.0, 2.0),
            reference: tone([3.0, 48.0, 98.0], 24.0, 0.0, 6.0),
            a_diff: 1.0,
            b_diff: -4.0,
        };
        r.frames[0].references.push(ReferenceResult {
            tool: Tool::Nlp,
            de2000: DeStats {
                mean: 2.0,
                p95: 4.0,
                max: 9.0,
            },
            alignment: Some(Alignment {
                rotate_degrees: 270,
                flip_horizontal: true,
                scale: 1.25,
                angle_degrees: -0.75,
                offset: [10.0, 12.5],
                overlap: 0.64,
                correlation: 0.93,
                luminance_correlation: 0.99,
            }),
            tone: Some(stats),
        });
        r.frames[0].unaligned.push(UnalignedReference {
            tool: Tool::Nlp,
            file: "reference/r001-03.nlp.jpg".into(),
            alignment: None,
        });
        r.summary.de2000_mean_by_tool.nlp = Some(2.0);
        let means = ToneMeans::over(&[stats]).unwrap();
        r.summary.tone = Some(ToneSummary {
            all: means,
            negpy: None,
            negadoctor: None,
            nlp: Some(means),
        });
        r
    }

    #[test]
    fn the_angle_is_recorded_and_older_reports_read_it_as_zero() {
        let reg = Registration {
            orientation: crate::eval::align::ORIENTATIONS[0],
            scale: 1.0,
            offset: [1.0, 2.0],
            angle_degrees: 1.25,
            correlation: 0.9,
            luminance_correlation: 0.95,
            reference_size: (10, 10),
        };
        let a = Alignment::new(&reg, 20, 20);
        assert_eq!(a.angle_degrees, 1.25);
        let mut json = serde_json::to_value(a).unwrap();
        assert_eq!(json["angle_degrees"], 1.25);
        json.as_object_mut().unwrap().remove("angle_degrees");
        let old: Alignment = serde_json::from_value(json).unwrap();
        assert_eq!(old.angle_degrees, 0.0);
    }

    #[test]
    fn tone_means_are_signed_and_absolute() {
        let a = ToneStats {
            render: tone([5.0, 50.0, 95.0], 20.0, 1.0, 2.0),
            reference: tone([3.0, 48.0, 98.0], 24.0, 0.0, 6.0),
            a_diff: 3.0,
            b_diff: -4.0,
        };
        let b = ToneStats {
            render: tone([3.0, 48.0, 98.0], 26.0, 0.0, 0.0),
            reference: tone([5.0, 50.0, 95.0], 24.0, 0.0, 0.0),
            a_diff: 0.0,
            b_diff: 0.0,
        };
        assert_eq!(ToneMeans::over(&[]), None);
        let m = ToneMeans::over(&[a, b]).unwrap();
        assert_eq!(m.references, 2);
        // Opposite signed differences cancel; the absolute ones do not.
        assert_eq!((m.l_p1_diff, m.l_p50_diff, m.l_p99_diff), (0.0, 0.0, 0.0));
        assert!((m.l_abs_diff - 7.0 / 3.0).abs() < 1e-6, "{m:?}");
        assert_eq!((m.chroma_diff, m.chroma_abs_diff), (-1.0, 3.0));
        assert_eq!((m.a_diff, m.b_diff), (1.5, -2.0));
        // hypot(3, -4) = 5 and 0.
        assert!((m.cast_ab - 2.5).abs() < 1e-6, "{m:?}");
    }

    #[test]
    fn registered_references_round_trip_and_print() {
        let r = registered_report();
        let text = to_json(&r).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        let reference = &v["frames"][0]["references"][1];
        assert_eq!(reference["tool"], "nlp");
        assert_eq!(reference["alignment"]["rotate_degrees"], 270);
        assert_eq!(reference["tone"]["b_diff"], -4.0);
        // The first reference was compared pixel by pixel: no new keys.
        let first = v["frames"][0]["references"][0].as_object().unwrap();
        assert!(!first.contains_key("alignment") && !first.contains_key("tone"));
        assert_eq!(
            v["frames"][0]["unaligned"][0]["file"],
            "reference/r001-03.nlp.jpg"
        );
        assert_eq!(v["summary"]["tone"]["nlp"]["references"], 1);
        assert!(v["summary"]["tone"].get("negpy").is_none());
        let back: EvalReport = serde_json::from_str(&text).unwrap();
        assert_eq!(
            back.frames[0].references[1].alignment,
            r.frames[0].references[1].alignment
        );
        assert_eq!(back.summary, r.summary);

        let t = render_table(&r);
        assert!(
            t.contains("(dE mean by tool: negpy 0.0000, negadoctor -, nlp 2.0000)"),
            "{t}"
        );
        assert!(
            t.lines().any(|l| l.starts_with("r001-03")
                && l.contains("nlp")
                && l.contains("270")
                && l.contains("5.0/3.0")
                && l.ends_with("-4.00")),
            "{t}"
        );
        assert!(
            t.contains(
                "tone (render minus reference, 1 registered): L* p1 +2.00  p50 +2.00  p99 -3.00  \
                 C* -4.00  a* +1.00  b* -4.00  (|L*| 2.33, |C*| 4.00, cast 4.12)"
            ),
            "{t}"
        );
        assert!(!t.contains('\u{2014}') && !t.contains('\u{2013}'));
    }

    #[test]
    fn reports_without_the_registration_fields_load_and_write_none() {
        // one_frame_report has none of them: its JSON names none, and a
        // report written before they existed loads with them empty.
        let text = to_json(&one_frame_report()).unwrap();
        for key in ["alignment", "tone", "unaligned", "\"nlp\""] {
            assert!(!text.contains(key), "{key}");
        }
        let back: EvalReport = serde_json::from_str(&text).unwrap();
        assert!(back.frames[0].unaligned.is_empty());
        assert!(back.frames[0].references[0].tone.is_none());
        assert_eq!(back.summary.tone, None);
        assert_eq!(back.summary.de2000_mean_by_tool.nlp, None);
    }

    #[test]
    fn unaligned_reason_names_the_file_and_correlation() {
        let u = Skipped::unaligned("r001-01", "reference/x.nlp.jpg", Some(0.1234), 0.5);
        assert_eq!(
            u.reason,
            "unaligned: reference/x.nlp.jpg (best correlation 0.123 is below 0.50)"
        );
        assert!(!u.is_reference() && !u.is_frame());
        let none = Skipped::unaligned("r001-01", "reference/x.nlp.jpg", None, 0.5);
        assert!(
            none.reason
                .ends_with("(no placement covers enough of the render)")
        );
    }
}
