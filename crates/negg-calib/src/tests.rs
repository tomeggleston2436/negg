#![allow(clippy::needless_range_loop)]

use negg_types::profile::{
    CrosstalkTable, DensityFitTable, FlatFieldTable, PROFILE_SCHEMA_VERSION, RigMethod,
};

use super::*;

fn constant(w: usize, h: usize, v: [f32; 3]) -> Image {
    let mut img = Image::new(w, h);
    for y in 0..h {
        for x in 0..w {
            img.set_pixel(x, y, v);
        }
    }
    img
}

fn vignetted(w: usize, h: usize, peak: f32) -> Image {
    let mut img = Image::new(w, h);
    let diag = ((w * w + h * h) as f32).sqrt() / 2.0;
    let focal = diag / 0.25f32.tan();
    for y in 0..h {
        for x in 0..w {
            let dx = x as f32 + 0.5 - w as f32 / 2.0;
            let dy = y as f32 + 0.5 - h as f32 / 2.0;
            let r = (dx * dx + dy * dy).sqrt();
            let cos = focal / (focal * focal + r * r).sqrt();
            let f = peak * cos.powi(4);
            img.set_pixel(x, y, [f, f * 0.9, f * 0.8]);
        }
    }
    img
}

#[test]
fn blur_of_constant_grid_is_constant_to_the_edges() {
    let g = constant(23, 17, [0.4, 0.5, 0.6]);
    let b = gaussian_blur_grid(&g, 3.7);
    for p in b.data().as_chunks::<3>().0.iter() {
        assert!(
            (p[0] - 0.4).abs() < 1e-6 && (p[1] - 0.5).abs() < 1e-6 && (p[2] - 0.6).abs() < 1e-6
        );
    }
}

#[test]
fn blur_of_impulse_sums_to_one_and_wide_kernels_reflect() {
    for sigma in [1.0f32, 2.5, 30.0] {
        let mut g = Image::new(9, 7);
        g.set_pixel(4, 3, [1.0, 2.0, 0.0]);
        let b = gaussian_blur_grid(&g, sigma);
        let sums = b
            .data()
            .as_chunks::<3>()
            .0
            .iter()
            .fold([0.0f64; 3], |mut a, p| {
                for c in 0..3 {
                    a[c] += f64::from(p[c]);
                }
                a
            });
        assert!((sums[0] - 1.0).abs() < 1e-4, "sigma {sigma}: {sums:?}");
        assert!((sums[1] - 2.0).abs() < 2e-4);
        assert!(sums[2].abs() < 1e-9);
    }
}

#[test]
fn blur_reflection_maps_edges_as_specified() {
    assert_eq!(reflect(-1, 5), 0);
    assert_eq!(reflect(-2, 5), 1);
    assert_eq!(reflect(5, 5), 4);
    assert_eq!(reflect(6, 5), 3);
    assert_eq!(reflect(-6, 5), 4);
    assert_eq!(reflect(10, 5), 0);
}

#[test]
fn blur_with_nonpositive_sigma_is_identity() {
    let mut g = Image::new(4, 4);
    g.set_pixel(1, 1, [1.0, 0.0, 0.0]);
    assert_eq!(gaussian_blur_grid(&g, 0.0), g);
}

#[test]
fn flat_field_flattens_a_vignetted_flat_within_one_percent() {
    let flat = vignetted(256, 192, 0.8);
    let (ff, mean) = flat_field(&flat).unwrap();
    assert_eq!((ff.width, ff.height), (256, 192));
    assert!(mean[0] > 0.0);
    // block = 1 here, so the gain grid is the frame: apply it directly.
    let mut lo = f32::MAX;
    let mut hi = f32::MIN;
    for y in 0..192 {
        for x in 0..256 {
            let v = flat.pixel(x, y)[0] * ff.gain.pixel(x, y)[0];
            lo = lo.min(v);
            hi = hi.max(v);
        }
    }
    let mid = 0.5 * (lo + hi);
    assert!((hi - lo) / mid < 0.01, "range {lo}..{hi}");
}

#[test]
fn flat_field_gain_is_clamped_both_ways() {
    // Zero blur cells (all-zero neighbourhood) give gain 4.
    let zero = constant(8, 8, [0.0; 3]);
    let (ff, _) = flat_field(&zero).unwrap();
    assert!(ff.gain.data().iter().all(|&g| g == FLAT_GAIN_MAX));

    // Strong gradient: mean / blur reaches both clamps.
    let mut grad = Image::new(64, 4);
    for y in 0..4 {
        for x in 0..64 {
            let v = if x < 2 { 0.9 } else { 0.0005 };
            grad.set_pixel(x, y, [v, v, v]);
        }
    }
    let (ff, _) = flat_field(&grad).unwrap();
    let gains = ff.gain.data();
    assert!(
        gains
            .iter()
            .all(|&g| (FLAT_GAIN_MIN..=FLAT_GAIN_MAX).contains(&g))
    );
    assert!(gains.contains(&FLAT_GAIN_MIN));
    assert!(gains.contains(&FLAT_GAIN_MAX));
}

#[test]
fn flat_field_rejects_clipped_flat() {
    let flat = constant(64, 64, [0.99, 0.5, 0.5]);
    assert!(matches!(flat_field(&flat), Err(CalibError::Clipped(_))));
    // Exactly at the 1% limit passes: 1 of 100 cells.
    let mut ok = constant(10, 10, [0.5; 3]);
    ok.set_pixel(0, 0, [0.99, 0.5, 0.5]);
    assert!(flat_field(&ok).is_ok());
    ok.set_pixel(1, 0, [0.5, 0.99, 0.5]);
    assert!(matches!(flat_field(&ok), Err(CalibError::Clipped(_))));
}

#[test]
fn flat_field_rejects_empty_and_non_finite() {
    assert!(matches!(
        flat_field(&Image::new(0, 0)),
        Err(CalibError::Invalid(_))
    ));
    let mut f = constant(4, 4, [0.5; 3]);
    f.set_pixel(0, 0, [f32::NAN, 0.5, 0.5]);
    assert!(matches!(flat_field(&f), Err(CalibError::Invalid(_))));
}

#[test]
fn flat_field_of_uniform_flat_has_unit_gain_and_mean() {
    let (ff, mean) = flat_field(&constant(32, 24, [0.3, 0.4, 0.5])).unwrap();
    for (c, m) in [0.3f32, 0.4, 0.5].into_iter().enumerate() {
        assert!((mean[c] - m).abs() < 1e-6);
    }
    assert!(ff.gain.data().iter().all(|g| (g - 1.0).abs() < 1e-5));
}

fn sample_m() -> [[f32; 3]; 3] {
    [[1.0, 0.12, 0.02], [0.08, 1.0, 0.15], [0.01, 0.05, 1.0]]
}

fn responses_from(m: [[f32; 3]; 3], scale: [f32; 3]) -> [[f32; 3]; 3] {
    let mut r = [[0.0f32; 3]; 3];
    for j in 0..3 {
        for c in 0..3 {
            r[j][c] = m[c][j] * scale[j];
        }
    }
    r
}

#[test]
fn crosstalk_recovers_m_and_inverse_is_exact() {
    let m = sample_m();
    let got = crosstalk_matrix(responses_from(m, [0.6, 0.5, 0.3])).unwrap();
    for r in 0..3 {
        for c in 0..3 {
            assert!((got[r][c] - m[r][c]).abs() < 1e-6);
        }
    }
    let inv = unmix_matrix(got).unwrap();
    for r in 0..3 {
        for c in 0..3 {
            let p: f32 = (0..3).map(|k| inv[r][k] * got[k][c]).sum();
            let want = if r == c { 1.0 } else { 0.0 };
            assert!((p - want).abs() < 1e-5, "({r},{c}) = {p}");
        }
    }
}

#[test]
fn crosstalk_rejects_swapped_bands() {
    let mut r = responses_from(sample_m(), [0.6, 0.5, 0.3]);
    r.swap(0, 1);
    assert!(matches!(crosstalk_matrix(r), Err(CalibError::Invalid(_))));
}

#[test]
fn crosstalk_rejects_singular_and_non_finite() {
    // Identical columns after normalisation.
    let r = [[1.0, 0.9, 0.0], [0.9, 1.0, 0.0], [0.0, 0.0, 1.0]];
    assert!(crosstalk_matrix(r).is_ok());
    let r = [[1.0, 1.0, 0.0], [1.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    assert_eq!(crosstalk_matrix(r), Err(CalibError::Singular));
    let r = [[f32::NAN, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    assert!(matches!(crosstalk_matrix(r), Err(CalibError::Invalid(_))));
    assert_eq!(unmix_matrix([[0.0; 3]; 3]), Err(CalibError::Singular));
}

fn log10(v: f32) -> f32 {
    v.max(1e-6).log10()
}

fn synth(model: &DensityModel, linear: [f32; 3]) -> DensitySample {
    let mut density = [0.0; 3];
    for c in 0..3 {
        density[c] = model.a[c]
            + (0..3)
                .map(|k| model.g[c][k] * log10(linear[k]))
                .sum::<f32>();
    }
    DensitySample { linear, density }
}

fn full_model() -> DensityModel {
    DensityModel {
        a: [0.05, -0.02, 0.1],
        g: [[-1.05, 0.06, 0.02], [0.03, -0.95, 0.04], [0.01, 0.05, -1.1]],
    }
}

fn coloured_patches() -> Vec<[f32; 3]> {
    let mut v = Vec::new();
    for i in 0..12u32 {
        let t = i as f32;
        v.push([
            0.05 + 0.07 * (t * 1.3).sin().abs() + 0.02 * t / 12.0,
            0.08 + 0.06 * (t * 0.7 + 1.0).cos().abs() + 0.03 * (12.0 - t) / 12.0,
            0.04 + 0.08 * (t * 2.1 + 0.5).sin().abs() + 0.01 * t / 12.0,
        ]);
    }
    v
}

#[test]
fn density_fit_recovers_a_known_full_model() {
    let model = full_model();
    let samples: Vec<_> = coloured_patches()
        .into_iter()
        .map(|l| synth(&model, l))
        .collect();
    let fit = fit_density(&samples).unwrap();
    assert_eq!(fit.kind, DensityModelKind::Full);
    assert_eq!(fit.n_patches, 12);
    assert!(fit.residual_rms < 1e-4, "{}", fit.residual_rms);
    for c in 0..3 {
        assert!((fit.model.a[c] - model.a[c]).abs() < 1e-3);
        for k in 0..3 {
            assert!(
                (fit.model.g[c][k] - model.g[c][k]).abs() < 1e-3,
                "g[{c}][{k}]"
            );
        }
    }
}

#[test]
fn density_fit_neutral_wedge_falls_back_to_diagonal() {
    let model = DensityModel {
        a: [0.05, 0.0, -0.03],
        g: [[-1.05, 0.0, 0.0], [0.0, -0.95, 0.0], [0.0, 0.0, -1.1]],
    };
    let samples: Vec<_> = (0..10)
        .map(|i| {
            let v = 0.9 * 0.7f32.powi(i);
            synth(&model, [v, v * 0.9, v * 0.8])
        })
        .collect();
    let fit = fit_density(&samples).unwrap();
    assert_eq!(fit.kind, DensityModelKind::Diagonal);
    assert!(fit.condition > 100.0);
    for c in 0..3 {
        assert!((fit.model.a[c] - model.a[c]).abs() < 1e-3);
        for k in 0..3 {
            assert!((fit.model.g[c][k] - model.g[c][k]).abs() < 1e-3);
        }
    }
    assert!(fit.residual_rms < 1e-4);
}

#[test]
fn density_fit_few_patches_uses_diagonal_and_two_error() {
    let model = full_model();
    let patches = coloured_patches();
    let seven: Vec<_> = patches[..7].iter().map(|&l| synth(&model, l)).collect();
    assert_eq!(
        fit_density(&seven).unwrap().kind,
        DensityModelKind::Diagonal
    );
    let two: Vec<_> = patches[..2].iter().map(|&l| synth(&model, l)).collect();
    assert_eq!(
        fit_density(&two),
        Err(CalibError::TooFewPatches { need: 3, got: 2 })
    );
}

#[test]
fn density_fit_rejects_positive_slope_and_bad_samples() {
    let model = DensityModel {
        a: [0.0; 3],
        g: [[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, -1.0]],
    };
    let samples: Vec<_> = coloured_patches()
        .into_iter()
        .map(|l| synth(&model, l))
        .collect();
    assert!(matches!(fit_density(&samples), Err(CalibError::Invalid(_))));

    let mut bad: Vec<_> = coloured_patches()
        .into_iter()
        .map(|l| synth(&full_model(), l))
        .collect();
    bad[0].density[1] = f32::NAN;
    assert!(matches!(fit_density(&bad), Err(CalibError::Invalid(_))));
}

#[test]
fn density_fit_without_spread_is_singular() {
    let s = DensitySample {
        linear: [0.5; 3],
        density: [0.3; 3],
    };
    assert_eq!(fit_density(&[s, s, s]), Err(CalibError::Singular));
}

fn table_source() -> String {
    "synthetic test rig".into()
}

fn profile(flat: Option<(u32, u32)>, with_m: bool, with_fit: bool) -> RigProfile {
    let mut method = Vec::new();
    let flat_field = flat.map(|(w, h)| {
        method.push(RigMethod::FlatField);
        FlatFieldTable {
            method: RigMethod::FlatField,
            source: table_source(),
            measured_on: "2026-01-02".into(),
            frame: "flat.tif".into(),
            mean: [0.5; 3],
            blur_sigma: "long_side / 64".into(),
            width: w,
            height: h,
        }
    });
    let crosstalk = with_m.then(|| {
        method.push(RigMethod::Crosstalk);
        CrosstalkTable {
            method: RigMethod::Crosstalk,
            source: table_source(),
            measured_on: "2026-01-02".into(),
            frame_r: "r.tif".into(),
            frame_g: "g.tif".into(),
            frame_b: "b.tif".into(),
            m: sample_m(),
        }
    });
    let density_fit = with_fit.then(|| {
        method.push(RigMethod::DensityFit);
        let m = full_model();
        DensityFitTable {
            method: RigMethod::DensityFit,
            source: table_source(),
            measured_on: "2026-01-02".into(),
            residual_rms: 0.001,
            n_patches: 12,
            model: DensityModelKind::Full,
            a: m.a,
            b: [m.g[0][0], m.g[1][1], m.g[2][2]],
            c: [
                [0.0, m.g[0][1], m.g[0][2]],
                [m.g[1][0], 0.0, m.g[1][2]],
                [m.g[2][0], m.g[2][1], 0.0],
            ],
        }
    });
    RigProfile {
        schema_version: PROFILE_SCHEMA_VERSION,
        name: "test-rig".into(),
        source: table_source(),
        measured_on: "2026-01-02".into(),
        method,
        residual_rms: if with_fit { 0.001 } else { 0.0 },
        camera: None,
        flat_field,
        crosstalk,
        density_fit,
    }
}

#[test]
fn rig_correction_builds_every_part() {
    let flat = vignetted(64, 48, 0.8);
    let p = profile(Some((64, 48)), true, true);
    let rc = rig_correction(&p, Some(&flat)).unwrap();
    assert_eq!(rc.name, "test-rig");
    let f = rc.flat.unwrap();
    assert_eq!((f.width, f.height), (64, 48));
    let inv = rc.unmix.unwrap();
    let m = sample_m();
    let p00: f32 = (0..3).map(|k| inv[0][k] * m[k][0]).sum();
    assert!((p00 - 1.0).abs() < 1e-5);
    assert_eq!(rc.density.unwrap(), p.density_model().unwrap());
}

#[test]
fn rig_correction_parts_are_optional() {
    let p = profile(None, true, false);
    let rc = rig_correction(&p, None).unwrap();
    assert!(rc.flat.is_none() && rc.density.is_none() && rc.unmix.is_some());
    // An unneeded flat frame is ignored.
    assert!(rig_correction(&p, Some(&constant(4, 4, [0.5; 3]))).is_ok());
}

#[test]
fn rig_correction_flat_errors() {
    let p = profile(Some((64, 48)), false, false);
    assert!(matches!(
        rig_correction(&p, None),
        Err(CalibError::Invalid(_))
    ));
    let wrong = constant(32, 48, [0.5; 3]);
    assert_eq!(
        rig_correction(&p, Some(&wrong)),
        Err(CalibError::SizeMismatch {
            expected: (64, 48),
            got: (32, 48)
        })
    );
    let clipped = constant(64, 48, [0.99; 3]);
    assert!(matches!(
        rig_correction(&p, Some(&clipped)),
        Err(CalibError::Clipped(_))
    ));
}

#[test]
fn rig_correction_rejects_invalid_profile() {
    let mut p = profile(None, true, false);
    p.source = "TODO(measure)".into();
    assert!(matches!(
        rig_correction(&p, None),
        Err(CalibError::Invalid(_))
    ));
}

#[test]
fn errors_display() {
    assert!(CalibError::Singular.to_string().contains("singular"));
    assert!(
        CalibError::TooFewPatches { need: 3, got: 1 }
            .to_string()
            .contains("need 3")
    );
    assert!(
        CalibError::SizeMismatch {
            expected: (1, 2),
            got: (3, 4)
        }
        .to_string()
        .contains("1x2")
    );
}
