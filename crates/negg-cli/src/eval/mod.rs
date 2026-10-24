// SPDX-License-Identifier: Apache-2.0

//! `negg eval`: render every corpus frame with default settings in roll
//! mode, score it and write `eval-results.json`.
//!
//! From the plan: "for each corpus frame render with default settings,
//! compute (a) CIEDE2000 against reference renders where present, (b)
//! grey-card neutrality error (mean |a*|, |b*| of the card patch), (c)
//! per-roll variance of estimated Dmin and gammas. Print a table and write
//! `eval-results.json`."
//!
//! A roll is the manifest `roll` field (Ruling 47), and every frame
//! belongs to one, so every frame is rendered in roll mode exactly as
//! `negg batch` renders a directory (docs/ADR/0017): the roll settings are
//! the defaults with the roll's `rig` and `film_profile` (paths relative
//! to the corpus directory) as its profiles; pass 1 decodes each present
//! frame of the roll in turn, applies the rig's step 2 corrections and
//! keeps its analysis ([`crate::roll::analyse_roll_frames`]), the frames
//! are pooled, and pass 2 decodes each frame again and renders it with the
//! pooled values ([`crate::roll::render_in_roll`]). Each frame is decoded
//! with `negg-decode`, brought to linear light and given its camera matrix
//! exactly as `negg convert` does (see [`crate::convert::prepare_input`]).
//! The render is linear Rec.2020 and is scored in L*a*b* through
//! [`negg_cms::lab::rec2020_linear_to_lab`]; reference renders are taken
//! to be sRGB-encoded (see [`metrics`]). A reference the same size as the
//! render is compared pixel by pixel. One of a different size is compared
//! on a shared analysis grid: both images go through
//! [`negg_film::analysis_grid`] with the long side set to the smaller of
//! the two long sides, and the statistics are taken over the block
//! medians; if the grids still differ by more than one cell in either
//! dimension the reference is registered onto the render instead
//! ([`align`], docs/ADR/0028) and scored over the overlap, and skipped as
//! a size mismatch when it does not register. A Negative Lab Pro
//! reference (tool `nlp`, a hand-cropped 8-bit JPEG or TIFF that may be
//! mirrored or turned) is always registered: it is scored over the
//! overlap with its placement and tone descriptors recorded, or listed as
//! `unaligned` and left out of every mean when its correlation is below
//! [`align::MIN_CORRELATION`]. A frame with a manifest `geometry` is
//! rendered with it as a frame override (the roll still pools), and the
//! geometry is applied to the render (step 8) before the references are
//! scored; the grey card is measured before that, on the uncropped render,
//! since its rect is given on the source frame. Neutrality is
//! measured on the full-resolution render. When a frame has both a
//! negadoctor and a NegPy reference, the two references are also compared
//! with each other ([`metrics::reference_distance`], matched in size the
//! same way; no distance when they cannot be matched), which is the
//! distance the plan's Gate 1 compares the engine's against; the summary
//! carries the Gate 1 fields and the per-tool means (Ruling 32).
//!
//! The per-roll statistics (Ruling 53, docs/ADR/0019) are taken over the
//! roll's Dmin inliers using each frame's own, unpooled Dmin and gamma
//! from pass 1: the population standard deviation and variance per
//! channel, present only when the roll has two or more inliers. The Gate 2
//! summary takes the largest Dmin standard deviation over the rolls and
//! the three-lights neutrality over the qualifying `film_roll`s; the
//! roll-to-roll spread of the pooled values per stock is reported beside
//! it.
//!
//! The corpus is local-only (docs/ADR/0005): the manifest is tracked but
//! the frames and reference renders are not, so any machine (CI above
//! all) may hold only a subset of them. A frame whose file is absent, or
//! which does not decode, is skipped and contributes nothing to the
//! frames, rolls, pools or summary; an evaluated frame whose reference
//! render is absent or mismatched in size has that one reference skipped
//! and the rest scored. Skips are counted in `corpus.skipped_frames` and
//! `corpus.skipped_references`, listed under `skipped` in the JSON with
//! their reasons, and reported after the table. A roll's profiles are
//! loaded only when at least one of its frames is present.
//!
//! With `--baseline`, the fresh report is also compared against a previous
//! `eval-results.json` using the tolerances in `eval.toml` (see
//! [`compare`] and [`tolerances`]); the comparison is printed after the
//! table and returned from [`run`], never written into the results file.

pub mod align;
pub mod compare;
pub mod error;
pub mod metrics;
pub mod report;
pub mod tolerances;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use negg_render::FrameRender;
use negg_types::{
    FrameOverrides, Image, PoolFrame, PoolRole, PooledEstimates, ProfileRefs, RollSettings,
    Settings,
};

use crate::cli::EvalArgs;
use crate::convert::ConvertError;
use crate::corpus::{self, Frame, Reference, Tool};
use crate::roll::{self, RollAnalysis};
pub use compare::Comparison;
pub use error::{BaselineError, EvalError};
use report::{
    Alignment, ByTool, CorpusSummary, EvalReport, FramePool, FrameResult, Gate1, Gate2,
    LightResult, ReferenceResult, RollResult, RollToRoll, Skipped, Summary, ThreeLights, ToneMeans,
    ToneSummary, UnalignedReference,
};

/// Loads the manifest, renders and scores every frame whose file is
/// present (roll by roll, in roll mode), writes the JSON to `args.out` and
/// prints the table to stdout unless `args.quiet`. Frames and reference
/// renders whose files are absent (or which cannot be used, see the module
/// documentation) are skipped, counted and listed rather than failing the
/// run; under `args.quiet` the one-line count is still printed when it is
/// not zero, so a thin local corpus never goes unnoticed. With
/// `args.baseline`, also loads the baseline and `args.tolerances` before
/// scoring (so a bad input fails fast), compares the fresh report against
/// the baseline and prints the comparison table after the results table.
/// Returns that comparison, or `None` without `args.baseline`.
///
/// # Errors
///
/// [`EvalError::Corpus`] if the manifest fails to load or validate,
/// [`EvalError::Baseline`] or [`EvalError::Tolerances`] if those inputs
/// fail to load, [`EvalError::Roll`] if a roll's rig or film profile
/// cannot be loaded (including its flat frame),
/// [`EvalError::Decode`] if a reference render that is present cannot be
/// read, [`EvalError::Cms`] or [`EvalError::Render`] if a decoded frame
/// cannot be linearised or rendered, or a roll's frames cannot be pooled
/// (a pooling error is a render error, see `roll_error`), [`EvalError::Io`] or
/// [`EvalError::Json`] if the results cannot be written.
pub fn run(args: &EvalArgs) -> Result<Option<Comparison>, EvalError> {
    let manifest = corpus::load(&args.corpus)?;
    let gate = match &args.baseline {
        Some(path) => Some((load_baseline(path)?, tolerances::load(&args.tolerances)?)),
        None => None,
    };

    let mut results: Vec<Option<FrameResult>> = vec![None; manifest.frame.len()];
    let mut pooled: HashMap<String, PooledEstimates> = HashMap::new();
    let mut skipped_frames = Vec::new();
    let mut skipped_references = Vec::new();
    for (roll_id, members) in rolls_in_order(&manifest.frame) {
        let mut present = Vec::with_capacity(members.len());
        for i in members {
            let frame = &manifest.frame[i];
            if args.corpus.join(&frame.file).is_file() {
                present.push(i);
            } else {
                skipped_frames.push(Skipped::frame(&frame.id, &frame.file));
            }
        }
        if present.is_empty() {
            continue;
        }
        let frames: Vec<&Frame> = present.iter().map(|&i| &manifest.frame[i]).collect();
        let outcome = render_roll(
            &args.corpus,
            roll_id,
            &frames,
            &mut skipped_frames,
            &mut skipped_references,
        )?;
        for (k, result) in outcome.frames {
            results[present[k]] = Some(result);
        }
        pooled.insert(roll_id.to_owned(), outcome.pooled);
    }
    let frames: Vec<FrameResult> = results.into_iter().flatten().collect();

    let report = assemble(
        frames,
        &manifest.frame,
        &pooled,
        skipped_frames,
        skipped_references,
    );
    std::fs::write(&args.out, report::to_json(&report)?)?;
    if !args.quiet {
        print!("{}", report::render_table(&report));
    } else if !report.skipped.is_empty() {
        println!("{}", report::skipped_line(&report));
    }

    let comparison = gate.map(|(baseline, tol)| compare::compare(&report, &baseline, &tol));
    if let Some(comparison) = &comparison
        && !args.quiet
    {
        print!("\n{}", compare::render_table(comparison));
    }
    Ok(comparison)
}

/// The manifest's rolls in order of first appearance, each with the
/// indices of its frames in manifest order.
fn rolls_in_order(frames: &[Frame]) -> Vec<(&str, Vec<usize>)> {
    let mut order: Vec<(&str, Vec<usize>)> = Vec::new();
    let mut index: HashMap<&str, usize> = HashMap::new();
    for (i, frame) in frames.iter().enumerate() {
        let slot = *index.entry(frame.roll.as_str()).or_insert_with(|| {
            order.push((frame.roll.as_str(), Vec::new()));
            order.len() - 1
        });
        order[slot].1.push(i);
    }
    order
}

/// Reads a previous `eval-results.json` to compare against. The schema
/// version is checked before the rest of the file is parsed, so a report
/// of another version (version 1 above all) is rejected with that reason
/// rather than with whatever field it lacks.
fn load_baseline(path: &Path) -> Result<EvalReport, EvalError> {
    let wrap = |source| EvalError::Baseline {
        path: path.to_owned(),
        source,
    };
    let text = std::fs::read_to_string(path).map_err(|e| wrap(BaselineError::Io(e)))?;
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| wrap(BaselineError::Json(e)))?;
    if let Some(found) = value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        && found != u64::from(report::REPORT_SCHEMA_VERSION)
    {
        return Err(wrap(BaselineError::SchemaVersion {
            found: u32::try_from(found).unwrap_or(u32::MAX),
        }));
    }
    let baseline: EvalReport =
        serde_json::from_value(value).map_err(|e| wrap(BaselineError::Json(e)))?;
    if baseline.schema_version != report::REPORT_SCHEMA_VERSION {
        return Err(wrap(BaselineError::SchemaVersion {
            found: baseline.schema_version,
        }));
    }
    Ok(baseline)
}

/// What [`render_roll`] produced for one roll.
struct RollOutcome {
    /// `(k, result)` for each frame scored, `k` indexing the frames passed
    /// in.
    frames: Vec<(usize, FrameResult)>,
    /// The roll's pooled values, filtered by its `pool_*` flags.
    pooled: PooledEstimates,
}

/// Maps an error of the roll machinery to the eval's own: linearisation and
/// render errors keep their eval variants, anything else is a roll error.
fn roll_error(roll: &str, err: ConvertError) -> EvalError {
    match err {
        ConvertError::Cms(e) => EvalError::Cms(e),
        ConvertError::Render(e) => EvalError::Render(e),
        source => EvalError::Roll {
            roll: roll.to_owned(),
            source,
        },
    }
}

/// Renders the present `frames` of one roll in roll mode, as `negg batch`
/// does a directory (ADR 0017), and scores each. The roll settings are the
/// defaults with the roll's `rig` and `film_profile` (the same for every
/// frame of the roll, checked by the manifest loader) resolved against
/// `corpus` (R-paths); a frame's only override is its manifest
/// `geometry`, as a whole (Ruling 71). A frame that does not
/// decode, in either pass, is appended to `skipped_frames` and left out of
/// the pool and the results.
fn render_roll(
    corpus: &Path,
    roll_id: &str,
    frames: &[&Frame],
    skipped_frames: &mut Vec<Skipped>,
    skipped_references: &mut Vec<Skipped>,
) -> Result<RollOutcome, EvalError> {
    let to_eval = |e| roll_error(roll_id, e);
    let mut roll_settings = RollSettings::default();
    roll_settings.shared.profiles = ProfileRefs {
        rig: frames[0].rig.clone(),
        film: frames[0].film_profile.clone(),
    };
    let ctx = roll::build_context(roll_settings, corpus, "negg eval").map_err(to_eval)?;
    let settings: Settings = ctx.roll.shared.clone();
    let inputs: Vec<(PathBuf, Settings)> = frames
        .iter()
        .map(|f| {
            let frame_settings = match &f.geometry {
                Some(g) => negg_types::roll::resolve(
                    &ctx.roll,
                    &FrameOverrides {
                        geometry: Some(g.to_geometry()),
                        ..FrameOverrides::default()
                    },
                ),
                None => settings.clone(),
            };
            (corpus.join(&f.file), frame_settings)
        })
        .collect();

    // Pass 1: analyse and pool. The pool records the analysed frames in
    // input order.
    let RollAnalysis {
        pool,
        frames: analyses,
    } = roll::analyse_roll_frames(&inputs, &ctx).map_err(to_eval)?;
    let mut records = pool.frames.iter();
    let mut analysed: Vec<(usize, &PoolFrame)> = Vec::with_capacity(frames.len());
    for (k, analysis) in analyses.into_iter().enumerate() {
        match analysis {
            Ok(_) => {
                let record = records.next().ok_or_else(|| {
                    to_eval(ConvertError::Invalid(
                        "the roll pool has fewer frames than were analysed".into(),
                    ))
                })?;
                analysed.push((k, record));
            }
            Err(ConvertError::Decode(err)) => {
                skipped_frames.push(Skipped::decode_failed(&frames[k].id, &err.to_string()));
            }
            Err(err) => return Err(to_eval(err)),
        }
    }

    // Pass 2: decode again, render with the pool and score.
    let mut results = Vec::with_capacity(analysed.len());
    for (k, record) in analysed {
        let frame = frames[k];
        let prepared = match roll::prepare_frame(&inputs[k].0, ctx.rig.as_ref()) {
            Ok(prepared) => prepared,
            Err(ConvertError::Decode(err)) => {
                skipped_frames.push(Skipped::decode_failed(&frame.id, &err.to_string()));
                continue;
            }
            Err(err) => return Err(to_eval(err)),
        };
        for note in &prepared.notes {
            eprintln!("negg eval: {}: {note}", frame.id);
        }
        let frame_settings = &inputs[k].1;
        let render =
            roll::render_in_roll(&prepared, frame_settings, &ctx, Some(&pool)).map_err(to_eval)?;
        drop(prepared);
        let pool_record = FramePool {
            dmin: record.dmin,
            gamma: record.gamma,
            dmin_role: record.dmin_role,
            gamma_role: record.gamma_role,
            cast_role: record.cast_role,
        };
        results.push((
            k,
            score_render(
                corpus,
                frame,
                render,
                &frame_settings.geometry,
                Some(pool_record),
                skipped_references,
            )?,
        ));
    }
    Ok(RollOutcome {
        frames: results,
        pooled: pool.pooled(&ctx.roll),
    })
}

/// Scores one rendered frame. Neutrality is measured on the render as it
/// comes from the engine, whenever the frame has a grey card (its rect is
/// on the source frame); then `geometry` is applied (step 8, Ruling 70)
/// and the references are scored against the result. A reference whose
/// file is absent, whose size cannot be matched to the render, or which
/// does not register onto it, is appended to `skipped` and the frame's
/// other references are still scored; with every reference skipped the
/// frame still gets a result, with no references.
fn score_render(
    corpus: &Path,
    frame: &Frame,
    mut render: FrameRender,
    geometry: &negg_types::Geometry,
    pool: Option<FramePool>,
    skipped: &mut Vec<Skipped>,
) -> Result<FrameResult, EvalError> {
    let neutrality = frame
        .grey_card
        .map(|card| metrics::neutrality(&render.image, card));
    if !geometry.is_identity() {
        render.image = geometry.apply(&render.image)?;
    }
    let mut references = Vec::with_capacity(frame.references.len());
    let mut unaligned = Vec::new();
    // The first scored NegPy and negadoctor references, kept for the
    // reference-to-reference distance.
    let mut negpy: Option<Image> = None;
    let mut negadoctor: Option<Image> = None;
    for reference in &frame.references {
        let path = corpus.join(&reference.file);
        if !path.is_file() {
            skipped.push(Skipped::reference(&frame.id, &reference.file));
            continue;
        }
        let image = read_reference(&path)?;
        let scored = match score_reference(&render.image, &image, reference) {
            Scored::Pair(r, s) => ReferenceResult {
                tool: reference.tool,
                de2000: metrics::de2000_stats(&r, &s).map_err(|e| e.for_frame(&frame.id))?,
                alignment: None,
                tone: None,
            },
            Scored::Registered(registration) => {
                let (de2000, tone) = metrics::aligned_stats(&render.image, &image, &registration)
                    .map_err(|e| e.for_frame(&frame.id))?;
                ReferenceResult {
                    tool: reference.tool,
                    de2000,
                    alignment: Some(Alignment::new(
                        &registration,
                        render.image.width(),
                        render.image.height(),
                    )),
                    tone: Some(tone),
                }
            }
            Scored::Unaligned(registration) => {
                let alignment = registration
                    .map(|r| Alignment::new(&r, render.image.width(), render.image.height()));
                skipped.push(Skipped::unaligned(
                    &frame.id,
                    &reference.file,
                    alignment.map(|a| a.correlation),
                    align::MIN_CORRELATION,
                ));
                unaligned.push(UnalignedReference {
                    tool: reference.tool,
                    file: reference.file.clone(),
                    alignment,
                });
                continue;
            }
            Scored::SizeMismatch => {
                skipped.push(Skipped::size_mismatch(
                    &frame.id,
                    &reference.file,
                    (render.image.width(), render.image.height()),
                    (image.width(), image.height()),
                ));
                continue;
            }
        };
        references.push(scored);
        let slot = match reference.tool {
            Tool::Negpy => &mut negpy,
            Tool::Negadoctor => &mut negadoctor,
            Tool::Nlp => continue,
        };
        if slot.is_none() {
            *slot = Some(image);
        }
    }
    let reference_distance = match (&negadoctor, &negpy) {
        (Some(nd), Some(np)) => match matched_pair(nd, np) {
            Some((a, b)) => {
                Some(metrics::reference_distance(&a, &b).map_err(|e| e.for_frame(&frame.id))?)
            }
            None => None,
        },
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
        estimates: Some(render.estimates),
        reference_distance,
        film_roll: frame.film_roll.clone(),
        rig: frame.rig.clone(),
        pool,
        unaligned,
    })
}

/// How one reference is to be scored against the render.
enum Scored {
    /// Pixel by pixel on these two images of one size: the originals when
    /// the sizes match, else the shared analysis grid.
    Pair(Image, Image),
    /// Over the overlap of this registration.
    Registered(align::Registration),
    /// A Negative Lab Pro reference that did not register, with the best
    /// placement found when there was one.
    Unaligned(Option<align::Registration>),
    /// A NegPy or negadoctor reference that neither matches in size nor
    /// registers.
    SizeMismatch,
}

/// Chooses the comparison for one reference (docs/ADR/0028). A NegPy or
/// negadoctor reference keeps the comparison it always had (pixel by
/// pixel at the render's size, else on the shared analysis grid) and is
/// registered only when neither applies; it is then a size mismatch,
/// exactly as before, when it does not register. A Negative Lab Pro
/// reference is always registered, at any size, and is unaligned when it
/// does not register.
fn score_reference(render: &Image, image: &Image, reference: &Reference) -> Scored {
    if reference.tool != Tool::Nlp
        && let Some((r, s)) = matched_pair(render, image)
    {
        return Scored::Pair(r, s);
    }
    let registration = align::register(render, image);
    match registration {
        Some(r) if r.is_aligned() => Scored::Registered(r),
        _ if reference.tool == Tool::Nlp => Scored::Unaligned(registration),
        _ => Scored::SizeMismatch,
    }
}

/// Reads a reference render: a JPEG (`.jpg` or `.jpeg`, any case), as a
/// Negative Lab Pro export usually is, or else a TIFF through
/// [`negg_decode::read_tiff`]. Either way the values are the file's
/// encoded ones scaled to `[0, 1]` and taken to be sRGB (see [`metrics`]);
/// a greyscale JPEG is read as grey RGB. Orientation tags are ignored,
/// since registration finds the orientation.
fn read_reference(path: &Path) -> Result<Image, EvalError> {
    let jpeg = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg"));
    if !jpeg {
        return Ok(negg_decode::read_tiff(path)?.image);
    }
    let wrap = |message: String| EvalError::Reference {
        path: path.to_owned(),
        message,
    };
    let file = std::fs::File::open(path).map_err(|e| wrap(e.to_string()))?;
    let decoder = image::codecs::jpeg::JpegDecoder::new(std::io::BufReader::new(file))
        .map_err(|e| wrap(e.to_string()))?;
    let img = image::DynamicImage::from_decoder(decoder).map_err(|e| wrap(e.to_string()))?;
    let rgb = match img {
        image::DynamicImage::ImageRgb8(rgb) => rgb,
        image::DynamicImage::ImageLuma8(_) => img.to_rgb8(),
        other => {
            return Err(wrap(format!(
                "colour type {:?}; only 8-bit RGB and greyscale JPEGs are read",
                other.color()
            )));
        }
    };
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let data: Vec<f32> = rgb
        .into_raw()
        .iter()
        .map(|&v| f32::from(v) / 255.0)
        .collect();
    Image::from_vec(w, h, data).map_err(EvalError::Render)
}

/// The render and reference as two images of one size: the originals
/// when they already match, else both reduced to block medians on a
/// shared analysis grid (long side = the smaller of the two long sides)
/// and cropped to the common cell count. `None` when the grids differ by
/// more than one cell in either dimension.
fn matched_pair(render: &Image, reference: &Image) -> Option<(Image, Image)> {
    if render.width() == reference.width() && render.height() == reference.height() {
        return Some((render.clone(), reference.clone()));
    }
    let long = |i: &Image| i.width().max(i.height());
    let long_side = long(render).min(long(reference));
    let r = negg_film::analysis_grid(render, long_side).median;
    let s = negg_film::analysis_grid(reference, long_side).median;
    if r.width().abs_diff(s.width()) > 1 || r.height().abs_diff(s.height()) > 1 {
        return None;
    }
    let w = r.width().min(s.width());
    let h = r.height().min(s.height());
    Some((crop(&r, w, h), crop(&s, w, h)))
}

/// The top-left `w` by `h` pixels of `image`.
fn crop(image: &Image, w: usize, h: usize) -> Image {
    if w == image.width() && h == image.height() {
        return image.clone();
    }
    let mut out = Image::new(w, h);
    for y in 0..h {
        for x in 0..w {
            out.set_pixel(x, y, image.pixel(x, y));
        }
    }
    out
}

/// Groups `items` by `key` in order of first appearance, leaving out the
/// items whose key is `None`.
fn group_in_order<T>(items: &[T], key: fn(&T) -> Option<&str>) -> Vec<(&str, Vec<&T>)> {
    let mut order: Vec<(&str, Vec<&T>)> = Vec::new();
    let mut index: HashMap<&str, usize> = HashMap::new();
    for item in items {
        let Some(k) = key(item) else { continue };
        let slot = *index.entry(k).or_insert_with(|| {
            order.push((k, Vec::new()));
            order.len() - 1
        });
        order[slot].1.push(item);
    }
    order
}

/// Builds the roll entries, the roll-to-roll and three-lights entries, the
/// summary and the skipped list from the per-frame results. `frames` holds
/// only the evaluated frames, in manifest order; `manifest_frames` is every
/// entry in the manifest, used for the total and to count grey cards on
/// the evaluated frames; `pooled` holds each rendered roll's pooled values.
fn assemble(
    frames: Vec<FrameResult>,
    manifest_frames: &[Frame],
    pooled: &HashMap<String, PooledEstimates>,
    skipped_frames: Vec<Skipped>,
    skipped_references: Vec<Skipped>,
) -> EvalReport {
    let evaluated: HashSet<&str> = frames.iter().map(|f| f.id.as_str()).collect();
    let corpus = CorpusSummary {
        frames: manifest_frames.len(),
        evaluated: frames.len(),
        with_references: frames.iter().filter(|f| !f.references.is_empty()).count(),
        with_grey_card: manifest_frames
            .iter()
            .filter(|f| f.has_grey_card && evaluated.contains(f.id.as_str()))
            .count(),
        skipped_frames: skipped_frames.len(),
        skipped_references: skipped_references.len(),
    };
    let mut skipped = skipped_frames;
    skipped.extend(skipped_references);
    skipped.sort();

    let by_roll = group_in_order(&frames, |f| Some(f.roll.as_str()));
    let rolls: Vec<RollResult> = by_roll
        .iter()
        .map(|(roll, members)| roll_result(roll, members, pooled.get(*roll)))
        .collect();
    let roll_to_roll = roll_to_roll(&by_roll, &rolls);
    let three_lights: Vec<ThreeLights> = group_in_order(&frames, |f| f.film_roll.as_deref())
        .into_iter()
        .map(|(film_roll, members)| three_lights(film_roll, &members))
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
    let tool_mean = |tool: Tool| {
        let means: Vec<f32> = frames
            .iter()
            .flat_map(|f| f.references.iter())
            .filter(|r| r.tool == tool)
            .map(|r| r.de2000.mean)
            .collect();
        mean(&means)
    };
    // Gate 1 over the frames with both references: the engine's distance
    // to the first NegPy reference and negadoctor's distance to it.
    let (engine_vs_negpy, negadoctor_vs_negpy): (Vec<f32>, Vec<f32>) = frames
        .iter()
        .filter_map(|f| {
            let distance = f.reference_distance?;
            let engine = f.references.iter().find(|r| r.tool == Tool::Negpy)?;
            Some((engine.de2000.mean, distance.mean))
        })
        .unzip();
    let neutrality_des: Vec<f32> = frames
        .iter()
        .filter_map(|f| f.neutrality)
        .map(|n| n.de)
        .collect();
    // Gate 2 (Ruling 53): the largest Dmin spread over the rolls with two
    // or more inliers, and the worst light over the qualifying film rolls.
    let dmin_std_max = max_of(rolls.iter().filter_map(|r| report::max_channel(r.dmin_std)));
    let three_lights_max_de = max_of(
        three_lights
            .iter()
            .filter(|t| t.qualifies)
            .filter_map(|t| t.max_de),
    );
    let summary = Summary {
        de2000_mean: mean(&de_means),
        de2000_p95_mean: mean(&de_p95s),
        neutrality_mean: mean(&neutralities),
        de2000_mean_by_tool: ByTool {
            negpy: tool_mean(Tool::Negpy),
            negadoctor: tool_mean(Tool::Negadoctor),
            nlp: tool_mean(Tool::Nlp),
        },
        gate1: Gate1::new(
            mean(&engine_vs_negpy),
            mean(&negadoctor_vs_negpy),
            mean(&neutrality_des),
        ),
        gate2: Gate2::new(dmin_std_max, three_lights_max_de),
        tone: tone_summary(&frames),
    };

    EvalReport {
        schema_version: report::REPORT_SCHEMA_VERSION,
        corpus,
        frames,
        rolls,
        roll_to_roll,
        three_lights,
        summary,
        skipped,
    }
}

/// The tone means over every registered reference and per tool
/// (docs/ADR/0028); `None` when no reference was registered.
fn tone_summary(frames: &[FrameResult]) -> Option<ToneSummary> {
    let tones = |tool: Option<Tool>| -> Vec<metrics::ToneStats> {
        frames
            .iter()
            .flat_map(|f| f.references.iter())
            .filter(|r| tool.is_none_or(|t| r.tool == t))
            .filter_map(|r| r.tone)
            .collect()
    };
    Some(ToneSummary {
        all: ToneMeans::over(&tones(None))?,
        negpy: ToneMeans::over(&tones(Some(Tool::Negpy))),
        negadoctor: ToneMeans::over(&tones(Some(Tool::Negadoctor))),
        nlp: ToneMeans::over(&tones(Some(Tool::Nlp))),
    })
}

/// One roll's entry: the statistics over its Dmin inliers' own values
/// (Ruling 53) and its pooled values.
fn roll_result(
    roll: &str,
    members: &[&FrameResult],
    pooled: Option<&PooledEstimates>,
) -> RollResult {
    let inliers: Vec<&FramePool> = members
        .iter()
        .filter_map(|f| f.pool.as_ref())
        .filter(|p| p.dmin_role == PoolRole::Inlier)
        .collect();
    let rejected = members
        .iter()
        .filter(|f| f.pool.is_some_and(|p| p.dmin_role == PoolRole::Rejected))
        .map(|f| f.id.clone())
        .collect();
    let dmin_variance = channel_variance(&inliers.iter().map(|p| p.dmin).collect::<Vec<_>>());
    let gamma_variance = channel_variance(&inliers.iter().map(|p| p.gamma).collect::<Vec<_>>());
    RollResult {
        roll: roll.to_owned(),
        frames: members.len(),
        dmin_variance,
        gamma_variance,
        inliers: inliers.len(),
        rejected,
        dmin_std: dmin_variance.map(sqrt_each),
        gamma_std: gamma_variance.map(sqrt_each),
        pooled_dmin: pooled.and_then(|p| p.dmin),
        pooled_gamma_ratio: pooled.and_then(|p| p.gamma).and_then(gamma_ratio),
    }
}

/// The pooled gamma as ratios to green, `[R/G, B/G]`; `None` when green is
/// not positive and finite.
fn gamma_ratio(gamma: [f32; 3]) -> Option<[f32; 2]> {
    (gamma[1].is_finite() && gamma[1] > 0.0).then(|| [gamma[0] / gamma[1], gamma[2] / gamma[1]])
}

/// The roll-to-roll entries (Ruling 53): per stock, in order of first
/// appearance, the spread across its rolls of the pooled Dmin and pooled
/// gamma ratios, counting only the rolls that have both; a stock needs two
/// such rolls. A roll's stock is that of its first evaluated frame.
fn roll_to_roll(by_roll: &[(&str, Vec<&FrameResult>)], rolls: &[RollResult]) -> Vec<RollToRoll> {
    // Per stock: the pooled Dmin and gamma ratios of each roll with both.
    type Pooled = ([f32; 3], [f32; 2]);
    let mut order: Vec<(&str, Vec<Pooled>)> = Vec::new();
    for ((_, members), roll) in by_roll.iter().zip(rolls) {
        let stock = members[0].stock.as_str();
        let slot = match order.iter().position(|(s, _)| *s == stock) {
            Some(slot) => slot,
            None => {
                order.push((stock, Vec::new()));
                order.len() - 1
            }
        };
        if let (Some(dmin), Some(ratio)) = (roll.pooled_dmin, roll.pooled_gamma_ratio) {
            order[slot].1.push((dmin, ratio));
        }
    }
    order
        .into_iter()
        .filter_map(|(stock, values)| {
            let dmins: Vec<[f32; 3]> = values.iter().map(|v| v.0).collect();
            let ratios: Vec<[f32; 2]> = values.iter().map(|v| v.1).collect();
            Some(RollToRoll {
                stock: stock.to_owned(),
                rolls: values.len(),
                dmin_std: sqrt_each(channel_variance(&dmins)?),
                gamma_ratio_std: sqrt_each(channel_variance(&ratios)?),
            })
        })
        .collect()
}

/// The three-lights entry of one film roll (Ruling 53): the mean card dE
/// per light source over its evaluated grey-card frames.
fn three_lights(film_roll: &str, members: &[&FrameResult]) -> ThreeLights {
    let cards: Vec<&FrameResult> = members
        .iter()
        .copied()
        .filter(|f| f.neutrality.is_some())
        .collect();
    let lights: Vec<LightResult> = group_in_order(&cards, |f| Some(f.light_source.as_str()))
        .into_iter()
        .map(|(light, frames)| {
            let des: Vec<f32> = frames
                .iter()
                .filter_map(|f| f.neutrality)
                .map(|n| n.de)
                .collect();
            LightResult {
                light_source: light.to_owned(),
                frames: frames.len(),
                neutrality_de_mean: mean(&des).unwrap_or(0.0),
            }
        })
        .collect();
    let qualifies =
        lights.len() >= report::GATE2_MIN_LIGHTS && cards.iter().all(|f| f.rig.is_some());
    let max_de = max_of(lights.iter().map(|l| l.neutrality_de_mean));
    ThreeLights {
        film_roll: film_roll.to_owned(),
        lights,
        qualifies,
        max_de,
    }
}

/// The largest value, or `None` when there is none.
fn max_of(values: impl Iterator<Item = f32>) -> Option<f32> {
    values.fold(None, |acc, v| Some(acc.map_or(v, |a: f32| a.max(v))))
}

/// The square root of each channel.
fn sqrt_each<const N: usize>(v: [f32; N]) -> [f32; N] {
    v.map(f32::sqrt)
}

/// Population variance per channel over the values, or `None` with fewer
/// than two of them.
fn channel_variance<const N: usize>(values: &[[f32; N]]) -> Option<[f32; N]> {
    if values.len() < 2 {
        return None;
    }
    let n = values.len() as f64;
    let mut out = [0.0f32; N];
    for (c, o) in out.iter_mut().enumerate() {
        let mean = values.iter().map(|v| f64::from(v[c])).sum::<f64>() / n;
        let var = values
            .iter()
            .map(|v| (f64::from(v[c]) - mean).powi(2))
            .sum::<f64>()
            / n;
        *o = var as f32;
    }
    Some(out)
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
    use negg_types::PooledEstimates;

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
                    alignment: None,
                    tone: None,
                })
                .collect(),
            neutrality: neutral,
            estimates: None,
            reference_distance: None,
            film_roll: None,
            rig: None,
            pool: None,
            unaligned: vec![],
        }
    }

    /// `f` with a pass 1 record: its own Dmin and gamma and its Dmin role.
    fn with_pool(
        mut f: FrameResult,
        dmin: [f32; 3],
        gamma: [f32; 3],
        role: PoolRole,
    ) -> FrameResult {
        f.pool = Some(FramePool {
            dmin,
            gamma,
            dmin_role: role,
            gamma_role: role,
            cast_role: role,
        });
        f
    }

    fn pooled(entries: &[(&str, [f32; 3], [f32; 3])]) -> HashMap<String, PooledEstimates> {
        entries
            .iter()
            .map(|&(roll, dmin, gamma)| {
                (
                    roll.to_owned(),
                    PooledEstimates {
                        dmin: Some(dmin),
                        gamma: Some(gamma),
                        cast: None,
                    },
                )
            })
            .collect()
    }

    /// A grey-card frame on film roll `film_roll` under `light`, with card
    /// dE `de` and a rig when `rig`.
    fn card(id: &str, roll: &str, film_roll: &str, light: &str, de: f32, rig: bool) -> FrameResult {
        let mut f = frame(
            id,
            roll,
            0,
            Some(Neutrality {
                a_mean_abs: de,
                b_mean_abs: de,
                de,
            }),
        );
        f.film_roll = Some(film_roll.into());
        f.light_source = light.into();
        f.rig = rig.then(|| "rig/r.rig.negg.toml".to_owned());
        f
    }

    /// A manifest entry with the given id, roll and grey-card flag.
    fn manifest_frame(id: &str, roll: &str, has_grey_card: bool) -> Frame {
        Frame {
            id: id.into(),
            file: format!("frames/{roll}/{id}.dng"),
            roll: roll.into(),
            stock: "s".into(),
            camera: "c".into(),
            light_source: "l".into(),
            has_grey_card,
            grey_card: None,
            references: vec![],
            notes: String::new(),
            contributor: String::new(),
            licence: "CC0-1.0".into(),
            film_roll: None,
            rig: None,
            film_profile: None,
            geometry: None,
        }
    }

    #[test]
    fn mean_of_empty_is_none() {
        assert_eq!(mean(&[]), None);
        assert_eq!(mean(&[1.0, 3.0]), Some(2.0));
    }

    #[test]
    fn channel_variance_is_population_variance_per_channel() {
        assert_eq!(channel_variance::<3>(&[]), None);
        assert_eq!(channel_variance(&[[1.0, 2.0, 3.0]]), None);
        // Channel 0: 1, 3 -> mean 2, population variance 1. Channel 1:
        // constant. Channel 2: 0, 0.3 -> 0.0225.
        let v = channel_variance(&[[1.0, 5.0, 0.0], [3.0, 5.0, 0.3]]).unwrap();
        assert!((v[0] - 1.0).abs() < 1e-6, "{v:?}");
        assert_eq!(v[1], 0.0);
        assert!((v[2] - 0.0225).abs() < 1e-6, "{v:?}");
    }

    #[test]
    fn assemble_counts_manifest_frames_and_skips_separately() {
        let manifest = vec![
            manifest_frame("r001-01", "r001", true),
            manifest_frame("r001-02", "r001", true),
            manifest_frame("r002-01", "r002", false),
        ];
        // r001-02 skipped entirely; r001-01 evaluated with one of its two
        // references skipped.
        let frames = vec![
            frame("r001-01", "r001", 1, None),
            frame("r002-01", "r002", 0, None),
        ];
        let skipped_frames = vec![Skipped::frame("r001-02", "frames/r001/r001-02.dng")];
        let skipped_references = vec![Skipped::reference(
            "r001-01",
            "reference/r001-01.negadoctor.tif",
        )];
        let r = assemble(
            frames,
            &manifest,
            &HashMap::new(),
            skipped_frames,
            skipped_references,
        );
        assert_eq!(r.corpus.frames, 3);
        assert_eq!(r.corpus.evaluated, 2);
        assert_eq!(r.corpus.with_references, 1);
        // Only the evaluated frame with a card counts, not the skipped one.
        assert_eq!(r.corpus.with_grey_card, 1);
        assert_eq!(r.corpus.skipped_frames, 1);
        assert_eq!(r.corpus.skipped_references, 1);
        assert_eq!(r.frames.len(), 2);
        assert_eq!(r.rolls.len(), 2);
        assert_eq!((r.rolls[0].roll.as_str(), r.rolls[0].frames), ("r001", 1));
        // Sorted by id then reason, regardless of insertion order.
        assert_eq!(
            r.skipped,
            vec![
                Skipped::reference("r001-01", "reference/r001-01.negadoctor.tif"),
                Skipped::frame("r001-02", "frames/r001/r001-02.dng"),
            ]
        );
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
                    de: 0.0,
                }),
            ),
            frame("b-02", "b", 1, None),
        ];
        let manifest = vec![
            manifest_frame("b-01", "b", false),
            manifest_frame("a-01", "a", false),
            manifest_frame("b-02", "b", false),
        ];
        let r = assemble(frames, &manifest, &HashMap::new(), vec![], vec![]);
        assert_eq!(r.corpus.frames, 3);
        assert_eq!(r.corpus.evaluated, 3);
        assert_eq!(r.corpus.with_references, 2);
        assert_eq!(r.corpus.with_grey_card, 0);
        assert_eq!(r.corpus.skipped_frames, 0);
        assert!(r.skipped.is_empty());
        assert_eq!(r.rolls.len(), 2);
        assert_eq!((r.rolls[0].roll.as_str(), r.rolls[0].frames), ("b", 2));
        assert_eq!((r.rolls[1].roll.as_str(), r.rolls[1].frames), ("a", 1));
        // No estimates on these frames: no variances.
        assert!(r.rolls[0].dmin_variance.is_none() && r.rolls[0].gamma_variance.is_none());
        // Reference means: 0, 1, 0 -> 1/3. p95s: 0, 2, 0 -> 2/3.
        assert!((r.summary.de2000_mean.unwrap() - 1.0 / 3.0).abs() < 1e-6);
        assert!((r.summary.de2000_p95_mean.unwrap() - 2.0 / 3.0).abs() < 1e-6);
        assert_eq!(r.summary.neutrality_mean, Some(2.0));
    }

    #[test]
    fn assemble_computes_roll_statistics_over_dmin_inliers() {
        let frames = vec![
            with_pool(
                frame("b-01", "b", 0, None),
                [0.2, 0.5, 0.7],
                [1.6, 1.7, 1.8],
                PoolRole::Inlier,
            ),
            with_pool(
                frame("a-01", "a", 0, None),
                [0.3, 0.5, 0.7],
                [1.6, 1.7, 1.8],
                PoolRole::Inlier,
            ),
            with_pool(
                frame("b-02", "b", 0, None),
                [0.4, 0.5, 0.7],
                [1.6, 1.9, 1.8],
                PoolRole::Inlier,
            ),
            // An outlier frame: Rejected from the Dmin pool, so it does not
            // move the statistics.
            with_pool(
                frame("b-03", "b", 0, None),
                [1.4, 0.5, 0.7],
                [0.6, 0.9, 0.8],
                PoolRole::Rejected,
            ),
        ];
        let manifest = vec![
            manifest_frame("b-01", "b", false),
            manifest_frame("a-01", "a", false),
            manifest_frame("b-02", "b", false),
            manifest_frame("b-03", "b", false),
        ];
        let pools = pooled(&[("b", [0.3, 0.5, 0.7], [1.6, 2.0, 1.8])]);
        let r = assemble(frames, &manifest, &pools, vec![], vec![]);
        let b = &r.rolls[0];
        assert_eq!(b.roll, "b");
        assert_eq!((b.frames, b.inliers), (3, 2));
        assert_eq!(b.rejected, vec!["b-03".to_owned()]);
        let dmin = b.dmin_variance.unwrap();
        // Channel 0: 0.2, 0.4 -> population variance 0.01, std 0.1.
        assert!((dmin[0] - 0.01).abs() < 1e-6, "{dmin:?}");
        assert_eq!(dmin[1], 0.0);
        assert_eq!(dmin[2], 0.0);
        let std = b.dmin_std.unwrap();
        assert!((std[0] - 0.1).abs() < 1e-6, "{std:?}");
        let gamma = b.gamma_variance.unwrap();
        assert_eq!(gamma[0], 0.0);
        assert!((gamma[1] - 0.01).abs() < 1e-6, "{gamma:?}");
        assert!((b.gamma_std.unwrap()[1] - 0.1).abs() < 1e-6);
        assert_eq!(b.pooled_dmin, Some([0.3, 0.5, 0.7]));
        let ratio = b.pooled_gamma_ratio.unwrap();
        assert!((ratio[0] - 0.8).abs() < 1e-6 && (ratio[1] - 0.9).abs() < 1e-6);
        // One inlier on roll a, and no pool: no statistics.
        let a = &r.rolls[1];
        assert_eq!(a.inliers, 1);
        assert_eq!(a.dmin_variance, None);
        assert_eq!(a.dmin_std, None);
        assert_eq!(a.pooled_dmin, None);
        // Gate 2 Dmin term: the largest channel of roll b's std.
        assert!((r.summary.gate2.dmin_std_max.unwrap() - 0.1).abs() < 1e-6);
        // No film rolls: no three-lights term, so no verdict.
        assert!(r.three_lights.is_empty());
        assert_eq!(r.summary.gate2.pass, None);
    }

    #[test]
    fn assemble_roll_to_roll_needs_two_pooled_rolls_of_a_stock() {
        let mut frames = vec![
            frame("r1-01", "r1", 0, None),
            frame("r2-01", "r2", 0, None),
            frame("r3-01", "r3", 0, None),
            frame("r4-01", "r4", 0, None),
        ];
        // r1, r2 and r4 are stock s; r3 is another stock with one roll.
        frames[2].stock = "t".into();
        let manifest: Vec<Frame> = frames
            .iter()
            .map(|f| manifest_frame(&f.id, &f.roll, false))
            .collect();
        // r4 has no pool and does not count.
        let pools = pooled(&[
            ("r1", [0.2, 0.5, 0.7], [1.6, 2.0, 1.8]),
            ("r2", [0.4, 0.5, 0.7], [1.6, 1.6, 1.8]),
            ("r3", [0.3, 0.5, 0.7], [1.6, 1.6, 1.6]),
        ]);
        let r = assemble(frames, &manifest, &pools, vec![], vec![]);
        assert_eq!(r.roll_to_roll.len(), 1);
        let s = &r.roll_to_roll[0];
        assert_eq!((s.stock.as_str(), s.rolls), ("s", 2));
        assert!((s.dmin_std[0] - 0.1).abs() < 1e-6, "{s:?}");
        assert_eq!(s.dmin_std[1], 0.0);
        // R/G: 0.8 and 1.0 -> std 0.1. B/G: 0.9 and 1.125 -> std 0.1125.
        assert!((s.gamma_ratio_std[0] - 0.1).abs() < 1e-6, "{s:?}");
        assert!((s.gamma_ratio_std[1] - 0.1125).abs() < 1e-6, "{s:?}");
    }

    #[test]
    fn assemble_three_lights_qualifies_with_three_lights_and_a_rig() {
        let frames = vec![
            // f001: three lights, every card frame with a rig: qualifies.
            card("a-01", "a", "f001", "L1", 1.0, true),
            card("a-02", "a", "f001", "L1", 2.0, true),
            card("b-01", "b", "f001", "L2", 0.5, true),
            card("c-01", "c", "f001", "L3", 1.2, true),
            // f002: three lights but one card frame without a rig.
            card("d-01", "d", "f002", "L1", 0.2, true),
            card("e-01", "e", "f002", "L2", 0.2, false),
            card("f-01", "f", "f002", "L3", 3.0, true),
            // f003: only two lights.
            card("g-01", "g", "f003", "L1", 5.0, true),
            card("h-01", "h", "f003", "L2", 5.0, true),
        ];
        let manifest: Vec<Frame> = frames
            .iter()
            .map(|f| manifest_frame(&f.id, &f.roll, true))
            .collect();
        let r = assemble(frames, &manifest, &HashMap::new(), vec![], vec![]);
        assert_eq!(r.three_lights.len(), 3);
        let f1 = &r.three_lights[0];
        assert_eq!(f1.film_roll, "f001");
        assert!(f1.qualifies);
        assert_eq!(f1.lights.len(), 3);
        assert_eq!(
            (f1.lights[0].light_source.as_str(), f1.lights[0].frames),
            ("L1", 2)
        );
        assert!((f1.lights[0].neutrality_de_mean - 1.5).abs() < 1e-6);
        assert!((f1.max_de.unwrap() - 1.5).abs() < 1e-6);
        let f2 = &r.three_lights[1];
        assert!(!f2.qualifies);
        assert!((f2.max_de.unwrap() - 3.0).abs() < 1e-6);
        assert!(!r.three_lights[2].qualifies);
        // Only f001 counts towards Gate 2.
        assert!((r.summary.gate2.three_lights_max_de.unwrap() - 1.5).abs() < 1e-6);
        // No roll has two inliers: no Dmin term, so no verdict.
        assert_eq!(r.summary.gate2.dmin_std_max, None);
        assert_eq!(r.summary.gate2.pass, None);
    }

    /// A roll with two Dmin inliers whose Dmin differ by `2 * std` in red,
    /// plus a qualifying film roll whose worst light has card dE `de`.
    fn gate2_report(std: f32, de: f32) -> EvalReport {
        let mut frames = vec![
            with_pool(
                frame("r-01", "r", 0, None),
                [0.3 - std, 0.5, 0.7],
                [1.6; 3],
                PoolRole::Inlier,
            ),
            with_pool(
                frame("r-02", "r", 0, None),
                [0.3 + std, 0.5, 0.7],
                [1.6; 3],
                PoolRole::Inlier,
            ),
        ];
        frames.push(card("a-01", "a", "f", "L1", de, true));
        frames.push(card("b-01", "b", "f", "L2", 0.1, true));
        frames.push(card("c-01", "c", "f", "L3", 0.1, true));
        let manifest: Vec<Frame> = frames
            .iter()
            .map(|f| manifest_frame(&f.id, &f.roll, f.neutrality.is_some()))
            .collect();
        assemble(frames, &manifest, &HashMap::new(), vec![], vec![])
    }

    #[test]
    fn assemble_gate2_pass_and_fail() {
        let pass = gate2_report(0.01, 1.0).summary.gate2;
        assert!((pass.dmin_std_max.unwrap() - 0.01).abs() < 1e-5, "{pass:?}");
        assert_eq!(pass.three_lights_max_de, Some(1.0));
        assert_eq!(pass.pass, Some(true));
        // Dmin spread over 0.02 log-D.
        assert_eq!(gate2_report(0.03, 1.0).summary.gate2.pass, Some(false));
        // Card dE at 2.
        assert_eq!(gate2_report(0.01, 2.0).summary.gate2.pass, Some(false));
    }

    #[test]
    fn assemble_empty_corpus() {
        let r = assemble(vec![], &[], &HashMap::new(), vec![], vec![]);
        assert_eq!(r.corpus.frames, 0);
        assert_eq!(r.corpus.evaluated, 0);
        assert!(r.rolls.is_empty());
        assert!(r.skipped.is_empty());
        assert_eq!(
            r.summary,
            Summary {
                de2000_mean: None,
                de2000_p95_mean: None,
                neutrality_mean: None,
                ..Summary::default()
            }
        );
    }

    fn ramp(w: usize, h: usize) -> Image {
        let mut img = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = (x + y) as f32 / (w + h) as f32;
                img.set_pixel(x, y, [v; 3]);
            }
        }
        img
    }

    #[test]
    fn matched_pair_keeps_equal_sizes_and_grids_unequal_ones() {
        let a = ramp(8, 6);
        let (r, s) = matched_pair(&a, &a).unwrap();
        assert_eq!(r, a);
        assert_eq!(s, a);

        // Twice the size: block 2 on the render, block 1 on the reference,
        // both 8x6.
        let big = ramp(16, 12);
        let (r, s) = matched_pair(&big, &a).unwrap();
        assert_eq!((r.width(), r.height()), (8, 6));
        assert_eq!((s.width(), s.height()), (8, 6));
        // The block median of a 2x2 ramp block equals the mean of its
        // middle values; the reference is the ramp itself.
        assert_eq!(s, a);

        // One cell off in a dimension (block 2 on 16x13 gives 8x7 against
        // the reference's 8x6): cropped to the smaller.
        let odd = ramp(16, 13);
        let (r, s) = matched_pair(&odd, &a).unwrap();
        assert_eq!((r.width(), r.height()), (s.width(), s.height()));
        assert_eq!((r.width(), r.height()), (8, 6));

        // Aspect ratios that do not land on the same grid: mismatch. So
        // does a width that forces a larger block (17 wide gives block 3
        // and a 6x4 grid).
        assert!(matched_pair(&ramp(17, 12), &a).is_none());
        assert!(matched_pair(&ramp(16, 16), &a).is_none());
        assert!(matched_pair(&ramp(64, 48), &ramp(16, 16)).is_none());
    }

    #[test]
    fn crop_takes_the_top_left() {
        let a = ramp(5, 4);
        let c = crop(&a, 3, 2);
        assert_eq!((c.width(), c.height()), (3, 2));
        assert_eq!(c.pixel(2, 1), a.pixel(2, 1));
        assert_eq!(crop(&a, 5, 4), a);
    }

    /// A grey scene with edges as a linear render (`encode` false) or
    /// sRGB-encoded (`encode` true), sampled at `offset + (x, y)`.
    fn scene(w: usize, h: usize, offset: [usize; 2], encode: bool) -> Image {
        let mut img = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let (sx, sy) = ((x + offset[0]) as f32, (y + offset[1]) as f32);
                let blob = (-((sx - 50.0).powi(2) + (sy - 30.0).powi(2)) / 300.0).exp();
                let bars = if (sx / 9.0).floor() as i32 % 3 == 0 {
                    0.1
                } else {
                    0.0
                };
                let v = 0.1 + 0.002 * sx + 0.003 * sy + 0.5 * blob + bars;
                let v = if encode {
                    negg_cms::lab::srgb_oetf(v)
                } else {
                    v
                };
                img.set_pixel(x, y, [v; 3]);
            }
        }
        img
    }

    fn reference(tool: Tool) -> Reference {
        Reference {
            tool,
            file: "reference/x".into(),
            version: "1".into(),
        }
    }

    #[test]
    fn score_reference_picks_the_comparison_by_tool_and_size() {
        let render = scene(120, 80, [0, 0], false);
        let same = scene(120, 80, [0, 0], true);
        let crop = scene(96, 64, [11, 7], true);
        // NegPy at the render's size: pixel by pixel, as before.
        assert!(matches!(
            score_reference(&render, &same, &reference(Tool::Negpy)),
            Scored::Pair(..)
        ));
        // A NegPy crop the grid cannot match: registered.
        match score_reference(&render, &crop, &reference(Tool::Negpy)) {
            Scored::Registered(r) => {
                assert!((r.offset[0] - 11.0).abs() < 1.0, "{r:?}");
                assert!((r.offset[1] - 7.0).abs() < 1.0, "{r:?}");
            }
            _ => panic!("expected a registration"),
        }
        // Negative Lab Pro is always registered, even at the same size.
        assert!(matches!(
            score_reference(&render, &same, &reference(Tool::Nlp)),
            Scored::Registered(_)
        ));
        // A flat reference registers nowhere: a size mismatch for NegPy,
        // unaligned for Negative Lab Pro.
        let mut flat = Image::new(50, 40);
        for y in 0..40 {
            for x in 0..50 {
                flat.set_pixel(x, y, [0.5; 3]);
            }
        }
        assert!(matches!(
            score_reference(&render, &flat, &reference(Tool::Negpy)),
            Scored::SizeMismatch
        ));
        assert!(matches!(
            score_reference(&render, &flat, &reference(Tool::Nlp)),
            Scored::Unaligned(None)
        ));
    }

    #[test]
    fn assemble_counts_nlp_references_and_their_tone() {
        let tone = metrics::ToneStats {
            render: metrics::ToneDescriptors {
                l_p1: 4.0,
                l_p50: 50.0,
                l_p99: 96.0,
                chroma_mean: 20.0,
                a_mean: 1.0,
                b_mean: 1.0,
            },
            reference: metrics::ToneDescriptors {
                l_p1: 2.0,
                l_p50: 50.0,
                l_p99: 99.0,
                chroma_mean: 25.0,
                a_mean: 0.0,
                b_mean: 0.0,
            },
            a_diff: 1.0,
            b_diff: 1.0,
        };
        let mut f = frame("r-01", "r", 1, None);
        f.references.push(ReferenceResult {
            tool: Tool::Nlp,
            de2000: DeStats {
                mean: 3.0,
                p95: 6.0,
                max: 9.0,
            },
            alignment: None,
            tone: Some(tone),
        });
        let manifest = vec![manifest_frame("r-01", "r", false)];
        let r = assemble(vec![f], &manifest, &HashMap::new(), vec![], vec![]);
        // The negpy reference (mean 0) and the nlp one (mean 3).
        assert_eq!(r.summary.de2000_mean, Some(1.5));
        assert_eq!(r.summary.de2000_mean_by_tool.nlp, Some(3.0));
        assert_eq!(r.summary.de2000_mean_by_tool.negpy, Some(0.0));
        let t = r.summary.tone.unwrap();
        assert_eq!(t.all.references, 1);
        assert_eq!(t.nlp, Some(t.all));
        assert_eq!(t.negpy, None);
        assert_eq!((t.all.l_p1_diff, t.all.chroma_diff), (2.0, -5.0));
        // Gate 1 is about NegPy alone and does not move.
        assert_eq!(r.summary.gate1.engine_vs_negpy, None);
    }
}
