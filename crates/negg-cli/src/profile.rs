// SPDX-License-Identifier: Apache-2.0

//! `negg profile rig` and `negg profile film`: measure a rig or film
//! profile and write it as `*.negg.toml` (docs/PROFILES.md; step 2 and
//! step 4 of docs/MATHS.md; ADRs 0014 to 0016).
//!
//! Orchestration only: the flat field, crosstalk and density fits are in
//! `negg-calib`, the patch statistic and neutral fit in `negg-film`, the
//! roll analysis in `negg-render`, and the TOML writers (which validate
//! every profile before writing it) in `negg-io`.
//!
//! Every measured number written is rounded to 6 significant digits first.
//! `measured_on` defaults to today's UTC date (Ruling 56).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use negg_calib::{
    DensityFit, DensitySample, crosstalk_matrix, fit_density, flat_field, unmix_matrix,
};
use negg_film::{analysis_grid, median, patch_median};
use negg_render::{ANALYSIS_GRID_LONG_SIDE, apply_rig_linear};
use negg_types::profile::{
    BaseTable, CrosstalkTable, DensityFitTable, DensityModelKind, DmaxTable, FilmMethod,
    FlatFieldTable, GammaTable, PROFILE_SCHEMA_VERSION, RigMethod, StockTable,
};
use negg_types::{FilmProfile, Image, PatchRect, PoolRole, RigCorrection, RigProfile, Settings};

use crate::cli::{ProfileCommand, ProfileFilmArgs, ProfileRigArgs};
use crate::convert::{ConvertError, batch_inputs};
use crate::roll::{RollContext, analyse_roll_frames, decode_linear, file_name, load_rig};

/// The central rect of a band frame whose patch median is its response
/// (R-crosstalk).
const BAND_RECT: PatchRect = PatchRect {
    x: 0.25,
    y: 0.25,
    width: 0.5,
    height: 0.5,
};

/// The full density model needs at least this many wedge patches
/// (R-density-fit); used only to word the fallback warning.
const FULL_MODEL_MIN_PATCHES: u32 = 8;

/// Runs a `negg profile` subcommand.
///
/// # Errors
///
/// See [`run_profile_rig`] and [`run_profile_film`].
pub fn run_profile(cmd: &ProfileCommand) -> Result<(), ConvertError> {
    match cmd {
        ProfileCommand::Rig(args) => run_profile_rig(args).map(|_| ()),
        ProfileCommand::Film(args) => run_profile_film(args).map(|_| ()),
    }
}

/// `v` rounded to 6 significant digits (zero and non-finite values are
/// returned as they are).
pub fn round6(v: f32) -> f32 {
    if v == 0.0 || !v.is_finite() {
        return v;
    }
    format!("{:.5e}", f64::from(v))
        .parse::<f64>()
        .map_or(v, |r| r as f32)
}

fn round3(v: [f32; 3]) -> [f32; 3] {
    v.map(round6)
}

fn round33(m: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    m.map(round3)
}

/// The proleptic Gregorian date of a day count since 1970-01-01, as
/// `(year, month, day)` (the civil-from-days algorithm).
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Today's UTC date as `YYYY-MM-DD` from the system clock (Ruling 56).
pub fn today_utc() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// `frame` relative to `dir`, with `/` separators, when it lies under it.
///
/// # Errors
///
/// [`ConvertError::Io`] when either path cannot be resolved and
/// [`ConvertError::Usage`] when `frame` is not under `dir`.
fn relative_to(frame: &Path, dir: &Path) -> Result<String, ConvertError> {
    let canon = |p: &Path| {
        std::fs::canonicalize(p).map_err(|source| ConvertError::Io {
            path: p.to_path_buf(),
            source,
        })
    };
    let (f, d) = (canon(frame)?, canon(dir)?);
    let rel = f.strip_prefix(&d).map_err(|_| {
        ConvertError::Usage(format!(
            "{} must lie under {}, the directory of the profile, because profile frame paths \
             are stored relative to it",
            frame.display(),
            d.display()
        ))
    })?;
    Ok(rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/"))
}

/// The directory a profile is written into, created if absent.
fn profile_dir(out: &Path) -> Result<PathBuf, ConvertError> {
    let abs = std::path::absolute(out).map_err(|source| ConvertError::Io {
        path: out.to_path_buf(),
        source,
    })?;
    let dir = abs
        .parent()
        .map_or_else(|| PathBuf::from("/"), Path::to_path_buf);
    std::fs::create_dir_all(&dir).map_err(|source| ConvertError::Io {
        path: dir.clone(),
        source,
    })?;
    Ok(dir)
}

/// The per-channel `patch_median` of `image`'s analysis grid over `rect`.
fn grid_patch(image: &Image, rect: PatchRect, what: &str) -> Result<[f32; 3], ConvertError> {
    let grid = analysis_grid(image, ANALYSIS_GRID_LONG_SIDE);
    patch_median(&grid.median, rect).ok_or_else(|| {
        ConvertError::Invalid(format!(
            "{what}: fewer than 4 analysis grid cells lie inside the patch {rect:?}"
        ))
    })
}

/// Why the density fit fell back to the diagonal model (R-density-fit),
/// or `None` for a full model: fewer than 8 patches, or a condition number
/// of the patch design above 100 (infinite for a neutral wedge, whose three
/// log channels are collinear). The condition number is only reported
/// here; the profile has no field for it, so a non-finite value is never
/// written.
pub fn diagonal_reason(fit: &DensityFit) -> Option<String> {
    if fit.kind != DensityModelKind::Diagonal {
        return None;
    }
    Some(if fit.n_patches < FULL_MODEL_MIN_PATCHES {
        format!(
            "only {} patches (the full model needs at least {FULL_MODEL_MIN_PATCHES})",
            fit.n_patches
        )
    } else if fit.condition.is_finite() {
        format!(
            "the condition number of the patch design is {:.1}, above 100",
            fit.condition
        )
    } else {
        "the patch design is rank deficient (infinite condition number), as for a neutral \
         wedge"
            .to_string()
    })
}

/// Runs `negg profile rig`: measures the flat field (R-flat), the crosstalk
/// matrix from three band frames (R-crosstalk, on the flat-corrected
/// frames) and the density model from a step wedge (R-density-fit, on the
/// flat-corrected and unmixed wedge), whichever are given, and writes the
/// rig profile. The flat and band frames must lie under the output file's
/// directory and are stored relative to it. A density fit that falls back
/// to the diagonal model prints a warning naming the reason. Returns the
/// profile written.
///
/// # Errors
///
/// [`ConvertError::Usage`] for an inconsistent command line or a frame
/// outside the profile's directory; [`ConvertError::Profile`] for a bad
/// wedge CSV or a profile that fails validation (for example an empty
/// `--source`); [`ConvertError::Calib`] when a fit fails; and the decode
/// errors of [`decode_linear`].
pub fn run_profile_rig(args: &ProfileRigArgs) -> Result<RigProfile, ConvertError> {
    let p = &args.provenance;
    let bands = [&args.band_r, &args.band_g, &args.band_b];
    let n_bands = bands.iter().filter(|b| b.is_some()).count();
    if n_bands != 0 && n_bands != 3 {
        return Err(ConvertError::Usage(
            "--band-r, --band-g and --band-b go together: give all three or none".into(),
        ));
    }
    if args.wedge.is_some() != args.densities.is_some() {
        return Err(ConvertError::Usage(
            "--wedge and --densities go together: give both or neither".into(),
        ));
    }
    if args.flat.is_none() && n_bands == 0 && args.wedge.is_none() {
        return Err(ConvertError::Usage(
            "nothing to measure: give --flat, the three --band-* frames, or --wedge with \
             --densities"
                .into(),
        ));
    }
    let measured_on = p.measured_on.clone().unwrap_or_else(today_utc);
    let dir = profile_dir(&p.out)?;
    let flat_rel = args
        .flat
        .as_deref()
        .map(|f| relative_to(f, &dir))
        .transpose()?;
    let band_paths: Option<[PathBuf; 3]> = match bands {
        [Some(r), Some(g), Some(b)] => Some([r.clone(), g.clone(), b.clone()]),
        _ => None,
    };
    let band_rel = band_paths
        .as_ref()
        .map(|b| -> Result<[String; 3], ConvertError> {
            Ok([
                relative_to(&b[0], &dir)?,
                relative_to(&b[1], &dir)?,
                relative_to(&b[2], &dir)?,
            ])
        })
        .transpose()?;
    let wedge_patches = args
        .densities
        .as_deref()
        .map(|d| {
            negg_io::read_wedge_csv(d).map_err(|source| ConvertError::Profile {
                path: d.to_path_buf(),
                source,
            })
        })
        .transpose()?;

    let mut camera = None;
    let mut rig = RigCorrection {
        name: p.name.clone(),
        flat: None,
        unmix: None,
        density: None,
    };
    let mut methods = Vec::new();

    // Step 2, flat field (R-flat).
    let flat_table = match (&args.flat, flat_rel) {
        (Some(path), Some(frame)) => {
            let (image, cam) = decode_linear(path)?;
            camera = camera.or(cam);
            let (flat, mean) = flat_field(&image)?;
            let table = FlatFieldTable {
                method: RigMethod::FlatField,
                source: p.source.clone(),
                measured_on: measured_on.clone(),
                frame,
                mean: round3(mean),
                blur_sigma: "long_side / 64".into(),
                width: u32::try_from(flat.width).unwrap_or(u32::MAX),
                height: u32::try_from(flat.height).unwrap_or(u32::MAX),
            };
            rig.flat = Some(flat);
            methods.push(RigMethod::FlatField);
            Some(table)
        }
        _ => None,
    };

    // Step 2, crosstalk (R-crosstalk) on the flat-corrected band frames.
    let crosstalk_table = match (band_paths, band_rel) {
        (Some(paths), Some([frame_r, frame_g, frame_b])) => {
            let mut responses = [[0.0f32; 3]; 3];
            for (j, path) in paths.iter().enumerate() {
                let (mut image, cam) = decode_linear(path)?;
                camera = camera.or(cam);
                apply_rig_linear(&mut image, &rig)?;
                responses[j] = grid_patch(&image, BAND_RECT, &path.display().to_string())?;
            }
            let m = crosstalk_matrix(responses)?;
            rig.unmix = Some(unmix_matrix(m)?);
            methods.push(RigMethod::Crosstalk);
            Some(CrosstalkTable {
                method: RigMethod::Crosstalk,
                source: p.source.clone(),
                measured_on: measured_on.clone(),
                frame_r,
                frame_g,
                frame_b,
                m: round33(m),
            })
        }
        _ => None,
    };

    // Step 2, density fit (R-density-fit) on the corrected wedge.
    let density_table = match (&args.wedge, wedge_patches) {
        (Some(path), Some(patches)) => {
            let (mut image, cam) = decode_linear(path)?;
            camera = camera.or(cam);
            apply_rig_linear(&mut image, &rig)?;
            let grid = analysis_grid(&image, ANALYSIS_GRID_LONG_SIDE);
            let samples = patches
                .iter()
                .map(|patch| {
                    let linear = patch_median(&grid.median, patch.rect).ok_or_else(|| {
                        ConvertError::Invalid(format!(
                            "{}: wedge patch {:?}: fewer than 4 analysis grid cells lie inside \
                             it",
                            path.display(),
                            patch.label
                        ))
                    })?;
                    Ok(DensitySample {
                        linear,
                        density: patch.density,
                    })
                })
                .collect::<Result<Vec<_>, ConvertError>>()?;
            let fit = fit_density(&samples)?;
            if let Some(reason) = diagonal_reason(&fit) {
                eprintln!(
                    "negg profile rig: warning: the density fit uses the diagonal model: {reason}"
                );
            }
            let g = fit.model.g;
            let mut c = g;
            for (i, row) in c.iter_mut().enumerate() {
                row[i] = 0.0;
            }
            methods.push(RigMethod::DensityFit);
            Some(DensityFitTable {
                method: RigMethod::DensityFit,
                source: p.source.clone(),
                measured_on: measured_on.clone(),
                residual_rms: round6(fit.residual_rms),
                n_patches: fit.n_patches,
                model: fit.kind,
                a: round3(fit.model.a),
                b: round3([g[0][0], g[1][1], g[2][2]]),
                c: round33(c),
            })
        }
        _ => None,
    };

    let profile = RigProfile {
        schema_version: PROFILE_SCHEMA_VERSION,
        name: p.name.clone(),
        source: p.source.clone(),
        measured_on,
        method: methods,
        residual_rms: density_table.as_ref().map_or(0.0, |t| t.residual_rms),
        camera,
        flat_field: flat_table,
        crosstalk: crosstalk_table,
        density_fit: density_table,
    };
    negg_io::write_rig_profile(&p.out, &profile).map_err(|source| ConvertError::Profile {
        path: p.out.clone(),
        source,
    })?;
    println!(
        "wrote {} ({}; residual_rms {:.6} log-D)",
        p.out.display(),
        profile
            .method
            .iter()
            .map(crate::convert::source_name)
            .collect::<Vec<_>>()
            .join(", "),
        profile.residual_rms
    );
    Ok(profile)
}

/// Runs `negg profile film`: pass 1 of roll mode over every batch input in
/// the directory (with the `--rig` correction, default settings and no
/// film correction), then for each grey patch of `patches.csv` the density
/// above the pooled base `d = max(patch_median(grid density) - Dmin, 0)`,
/// then [`negg_film::fit_neutral`] with `gamma_G` the pooled green gamma
/// (R-neutral-fit, R-contrast), and writes a `grey_card_roll` film profile
/// (R-film-profile). Returns the profile written.
///
/// # Errors
///
/// [`ConvertError::Profile`] for a bad rig profile, patches CSV or a
/// profile that fails validation; [`ConvertError::Invalid`] when a patch
/// names a frame that is not a batch input or could not be analysed, a
/// patch has too few cells, the roll has no pooled Dmin or gamma, or the
/// neutral fit fails; and the errors of [`batch_inputs`].
pub fn run_profile_film(args: &ProfileFilmArgs) -> Result<FilmProfile, ConvertError> {
    let who = "negg profile film";
    let p = &args.provenance;
    let measured_on = p.measured_on.clone().unwrap_or_else(today_utc);
    let rig = args.rig.as_deref().map(load_rig).transpose()?;
    let rig_name = rig.as_ref().map(|r| r.name.clone());
    let patches =
        negg_io::read_patches_csv(&args.patches).map_err(|source| ConvertError::Profile {
            path: args.patches.clone(),
            source,
        })?;
    let inputs = batch_inputs(&args.dir)?;
    let ctx = RollContext {
        roll: negg_types::RollSettings::default(),
        rig,
        film: None,
    };
    let list: Vec<(PathBuf, Settings)> = inputs
        .iter()
        .map(|p| (p.clone(), Settings::default()))
        .collect();
    let analysis = analyse_roll_frames(&list, &ctx)?;
    for (input, outcome) in inputs.iter().zip(&analysis.frames) {
        if let Err(e) = outcome {
            eprintln!("{who}: {}: {e}; left out of the roll", input.display());
        }
    }
    let pool = &analysis.pool;
    let dmin = pool.dmin.ok_or_else(|| {
        ConvertError::Invalid(format!(
            "{}: the roll has no pooled film base (no Dmin inliers)",
            args.dir.display()
        ))
    })?;
    let gamma_g = pool.gamma.map(|g| g[1]).ok_or_else(|| {
        ConvertError::Invalid(format!(
            "{}: the roll has no pooled gamma",
            args.dir.display()
        ))
    })?;

    let names: Vec<String> = inputs.iter().map(|p| file_name(p)).collect();
    let mut d_above_base = Vec::with_capacity(patches.len());
    for patch in &patches {
        let index = names.iter().position(|n| *n == patch.file).ok_or_else(|| {
            ConvertError::Invalid(format!(
                "{}: patch file {:?} is not a negative in {}",
                args.patches.display(),
                patch.file,
                args.dir.display()
            ))
        })?;
        let frame = analysis.frames[index].as_ref().map_err(|e| {
            ConvertError::Invalid(format!("patch file {:?} was not analysed: {e}", patch.file))
        })?;
        let d = patch_median(&frame.grid_density, patch.rect).ok_or_else(|| {
            ConvertError::Invalid(format!(
                "{}: patch {:?} of {:?}: fewer than 4 analysis grid cells lie inside it",
                args.patches.display(),
                patch.rect,
                patch.file
            ))
        })?;
        d_above_base.push([0, 1, 2].map(|c| (d[c] - dmin[c]).max(0.0)));
    }
    let fit = negg_film::fit_neutral(&d_above_base, gamma_g)
        .map_err(|e| ConvertError::Invalid(e.to_string()))?;

    let inliers: Vec<&negg_types::PoolFrame> = pool
        .frames
        .iter()
        .filter(|f| f.dmin_role == PoolRole::Inlier)
        .collect();
    let dmax = [0, 1, 2].map(|c| {
        let mut v: Vec<f32> = inliers.iter().map(|f| f.dmax[c]).collect();
        median(&mut v)
    });
    let rig_note = rig_name
        .as_deref()
        .map_or_else(|| "no rig profile".to_string(), |r| format!("rig {r:?}"));
    let roll_note = format!(
        "{} Dmin inlier frames of {} in {}, {rig_note}",
        inliers.len(),
        pool.frames.len(),
        args.dir.display()
    );
    let ratio = fit.ratio;
    let profile = FilmProfile {
        schema_version: PROFILE_SCHEMA_VERSION,
        name: p.name.clone(),
        source: p.source.clone(),
        measured_on,
        method: FilmMethod::GreyCardRoll,
        residual_rms: round6(fit.residual_rms),
        rig: rig_name,
        stock: StockTable {
            name: args.stock.clone(),
            process: "C-41".into(),
            iso: args.iso,
        },
        base: Some(BaseTable {
            dmin: round3(dmin),
            source: format!("roll-pooled film base over {roll_note}; {}", p.source),
        }),
        dmax: Some(DmaxTable {
            dmax: round3(dmax),
            source: format!("median Dmax of {roll_note}; {}", p.source),
        }),
        gamma: Some(GammaTable {
            gamma: round3(ratio.map(|r| r * gamma_g)),
            ratio: round3(ratio),
            offset: round3(fit.offset_log_d),
            gamma_source: "neutral_fit".into(),
            n_patches: fit.patches,
            source: format!(
                "neutral fit of {} grey patches from {} with the roll-pooled gamma_G {gamma_g:.4} \
                 over {roll_note}; {}",
                patches.len(),
                args.patches.display(),
                p.source
            ),
        }),
    };
    negg_io::write_film_profile(&p.out, &profile).map_err(|source| ConvertError::Profile {
        path: p.out.clone(),
        source,
    })?;
    println!(
        "wrote {} (ratio [{:.4}, {:.4}, {:.4}], offset [{:.4}, {:.4}, {:.4}] log-D, \
         residual_rms {:.6} log-D, {} points)",
        p.out.display(),
        ratio[0],
        ratio[1],
        ratio[2],
        fit.offset_log_d[0],
        fit.offset_log_d[1],
        fit.offset_log_d[2],
        fit.residual_rms,
        fit.patches
    );
    Ok(profile)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_from_day_counts() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(31), (1970, 2, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(20_749), (2026, 10, 23));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        let today = today_utc();
        assert_eq!(today.len(), 10, "{today}");
        assert_eq!(&today[4..5], "-");
    }

    #[test]
    fn the_diagonal_fallback_names_its_reason() {
        let fit = |kind, n_patches, condition| DensityFit {
            model: negg_types::DensityModel::IDENTITY,
            kind,
            residual_rms: 0.0,
            n_patches,
            condition,
        };
        assert_eq!(diagonal_reason(&fit(DensityModelKind::Full, 12, 3.0)), None);
        let r = diagonal_reason(&fit(DensityModelKind::Diagonal, 5, 3.0)).unwrap();
        assert!(r.starts_with("only 5 patches"), "{r}");
        let r = diagonal_reason(&fit(DensityModelKind::Diagonal, 12, 250.0)).unwrap();
        assert!(r.contains("250.0, above 100"), "{r}");
        let r = diagonal_reason(&fit(DensityModelKind::Diagonal, 12, f32::INFINITY)).unwrap();
        assert!(r.contains("rank deficient"), "{r}");
    }

    #[test]
    fn rounding_keeps_six_significant_digits() {
        assert_eq!(round6(0.0), 0.0);
        assert_eq!(round6(1.0), 1.0);
        assert_eq!(round6(-1.234_567_8), -1.234_57);
        assert_eq!(round6(0.0125 + 2e-9), 0.0125);
        assert_eq!(round6(0.0125 + 2e-9).to_string(), "0.0125");
        assert_eq!(round6(123_456_789.0), 123_457_000.0);
        assert!(round6(f32::INFINITY).is_infinite());
    }

    #[test]
    fn frames_must_lie_under_the_profile_directory() {
        let dir = std::env::temp_dir().join(format!("negg-profile-rel-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("frames")).unwrap();
        let f = dir.join("frames/flat.tif");
        std::fs::write(&f, b"x").unwrap();
        assert_eq!(relative_to(&f, &dir).unwrap(), "frames/flat.tif");
        let err = relative_to(&f, &dir.join("frames/../frames/sub_missing")).unwrap_err();
        assert!(matches!(err, ConvertError::Io { .. }), "{err}");
        std::fs::create_dir_all(dir.join("other")).unwrap();
        let err = relative_to(&f, &dir.join("other")).unwrap_err();
        assert!(err.is_usage_error(), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
