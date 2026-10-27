// SPDX-License-Identifier: MPL-2.0

//! The GPU preview kernel against the CPU kernel (ADR 0036): the positive
//! within `gpu::TOLERANCE` per channel, and the display encoding within one
//! 8-bit step of the CPU tile encoded by Little-CMS. Runs only with the
//! `gpu` feature. Without an adapter each test says so and passes, unless
//! `NEGG_REQUIRE_GPU` is `1` (CI sets it on Linux, where it installs a
//! software Vulkan adapter), when it fails.

#![cfg(feature = "gpu")]

use negg_render::gpu::{Gpu, TOLERANCE};
use negg_render::{PrintCoefficients, Region, render_region, solve_x0};
use negg_types::settings::{OutputSpace, PrintSettings};
use negg_types::{
    CastCorrection, CastSource, DensityModel, DminSource, Estimates, GammaSource, Image,
};

fn gpu() -> Option<Gpu> {
    match Gpu::new() {
        Ok(g) => {
            eprintln!("GPU: {}", g.adapter());
            Some(g)
        }
        Err(e) if !std::env::var("NEGG_REQUIRE_GPU").is_ok_and(|v| v == "1") => {
            eprintln!("skipped, {e}");
            None
        }
        Err(e) => panic!("NEGG_REQUIRE_GPU is 1 but {e}"),
    }
}

/// A small xorshift generator, so the images have a fixed seed.
struct XorShift(u32);

impl XorShift {
    fn next_unit(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }
}

/// Linear transmittance whose density is uniform in `[0, 3.2]`, with a few
/// samples at and below the transmittance floor.
fn random_linear(w: usize, h: usize, seed: u32) -> Image {
    let mut rng = XorShift(seed);
    let mut img = Image::new(w, h);
    for (i, v) in img.data_mut().iter_mut().enumerate() {
        *v = match i % 997 {
            0 => 0.0,
            1 => 1e-7,
            _ => 10f32.powf(-3.2 * rng.next_unit()),
        };
    }
    img
}

fn estimates() -> Estimates {
    Estimates {
        dmin: [0.21, 0.55, 0.78],
        dmin_source: DminSource::Border,
        dmax: [1.9, 2.2, 2.4],
        gamma: [1.69, 1.65, 1.62],
        gamma_source: GammaSource::Range,
        neutral_offset: [0.031, 0.0, -0.022],
        anchor: -0.42,
        x0: 0.0,
        cast: CastCorrection {
            slope_r: 1.05,
            offset_r: -0.01,
            slope_b: 0.97,
            offset_b: 0.02,
            cells_used: 40,
        },
        cast_source: CastSource::Fit,
    }
}

const MATRIX: [[f32; 3]; 3] = [[0.9, 0.1, 0.0], [0.05, 0.9, 0.05], [0.0, 0.2, 0.8]];

fn model() -> DensityModel {
    DensityModel {
        a: [0.05, -0.02, 0.1],
        g: [[-1.05, 0.04, 0.01], [0.03, -0.95, 0.02], [0.0, 0.06, -1.1]],
    }
}

/// Print settings covering every branch of the kernel.
fn prints() -> Vec<PrintSettings> {
    let base = PrintSettings::default();
    vec![
        base.clone(),
        PrintSettings {
            exposure: 0.1,
            cmy: [0.02, 0.0, -0.03],
            black_point_compensation: true,
            ..base.clone()
        },
        PrintSettings {
            grade: 160.0,
            toe: 3.0,
            shoulder: 9.0,
            saturation: 1.6,
            ..base.clone()
        },
        PrintSettings {
            grade: 70.0,
            saturation: 0.0,
            ..base
        },
    ]
}

/// One render's inputs: estimates, print coefficients, density model and
/// entry matrix.
type Case = (
    Estimates,
    PrintCoefficients,
    Option<DensityModel>,
    Option<[[f32; 3]; 3]>,
);

/// Every combination of print settings, density model and entry matrix.
fn cases() -> Vec<Case> {
    let mut out = Vec::new();
    for settings in prints() {
        for density in [None, Some(model())] {
            for matrix in [None, Some(MATRIX)] {
                let mut est = estimates();
                est.x0 = solve_x0(est.anchor, &settings);
                let c = PrintCoefficients::new(&settings, est.x0).unwrap();
                out.push((est, c, density, matrix));
            }
        }
    }
    out
}

#[test]
fn the_gpu_positive_matches_the_cpu_within_the_tolerance() {
    let Some(gpu) = gpu() else { return };
    let image = random_linear(300, 220, 0x9E37_79B9);
    let region = Region {
        x: 37,
        y: 11,
        width: 251,
        height: 190,
    };
    for (n, (est, c, density, matrix)) in cases().into_iter().enumerate() {
        let cpu = render_region(&image, region, &est, &c, density.as_ref(), matrix).unwrap();
        let ours = gpu
            .render_region(&image, region, &est, &c, density.as_ref(), matrix)
            .unwrap();
        assert_eq!((ours.width(), ours.height()), (cpu.width(), cpu.height()));
        let worst = cpu
            .data()
            .iter()
            .zip(ours.data())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(worst <= TOLERANCE, "case {n}: worst difference {worst}");
    }
}

#[test]
fn the_gpu_display_tile_is_within_one_step_of_the_cpu_tile() {
    let Some(gpu) = gpu() else { return };
    let srgb =
        negg_cms::working_to_output(OutputSpace::Srgb, negg_cms::Intent::Perceptual).unwrap();
    let display = negg_cms::working_to_output_matrix(OutputSpace::Srgb).unwrap();
    let image = random_linear(512, 512, 0x1234_5678);
    let region = Region {
        x: 0,
        y: 0,
        width: 512,
        height: 512,
    };
    for (n, (est, c, density, matrix)) in cases().into_iter().enumerate() {
        let cpu = srgb
            .apply_u8(&render_region(&image, region, &est, &c, density.as_ref(), matrix).unwrap());
        let ours = gpu
            .render_region_display(&image, region, &est, &c, density.as_ref(), matrix, display)
            .unwrap();
        assert_eq!(ours.len(), 512 * 512 * 4);
        let mut differ = 0usize;
        for (i, px) in ours.as_chunks::<4>().0.iter().enumerate() {
            assert_eq!(px[3], 255);
            for ch in 0..3 {
                let d = px[ch].abs_diff(cpu[i * 3 + ch]);
                assert!(
                    d <= 1,
                    "case {n} pixel {i} channel {ch}: GPU {} CPU {}",
                    px[ch],
                    cpu[i * 3 + ch]
                );
                differ += usize::from(d != 0);
            }
        }
        // Nearly every value is the same byte.
        assert!(
            differ * 100 <= 512 * 512 * 3,
            "case {n}: {differ} values differ"
        );
    }
}

#[test]
fn an_unusable_cast_slope_is_the_identity_on_the_gpu_too() {
    let Some(gpu) = gpu() else { return };
    let image = random_linear(64, 48, 7);
    let region = Region {
        x: 0,
        y: 0,
        width: 64,
        height: 48,
    };
    let mut est = estimates();
    est.cast.slope_r = 0.0;
    est.cast.slope_b = f32::NAN;
    let settings = PrintSettings::default();
    est.x0 = solve_x0(est.anchor, &settings);
    let c = PrintCoefficients::new(&settings, est.x0).unwrap();
    let cpu = render_region(&image, region, &est, &c, None, None).unwrap();
    let ours = gpu
        .render_region(&image, region, &est, &c, None, None)
        .unwrap();
    for (a, b) in cpu.data().iter().zip(ours.data()) {
        assert!((a - b).abs() <= TOLERANCE, "{a} {b}");
    }
}

#[test]
fn the_gpu_refuses_what_the_cpu_refuses() {
    let Some(gpu) = gpu() else { return };
    let image = random_linear(16, 16, 3);
    let mut est = estimates();
    est.gamma[1] = 0.0;
    let c = PrintCoefficients::new(&PrintSettings::default(), 0.0).unwrap();
    let whole = Region {
        x: 0,
        y: 0,
        width: 16,
        height: 16,
    };
    assert!(
        gpu.render_region(&image, whole, &est, &c, None, None)
            .is_err()
    );
    let est = estimates();
    let outside = Region {
        x: 8,
        y: 8,
        width: 16,
        height: 16,
    };
    assert!(
        gpu.render_region(&image, outside, &est, &c, None, None)
            .is_err()
    );
}

#[test]
fn nan_and_infinite_samples_render_as_the_cpu_renders_them() {
    let Some(gpu) = gpu() else { return };
    let mut image = random_linear(32, 8, 11);
    for (i, v) in image.data_mut().iter_mut().enumerate() {
        match i % 5 {
            0 => *v = f32::NAN,
            1 => *v = f32::INFINITY,
            2 => *v = f32::NEG_INFINITY,
            _ => {}
        }
    }
    let region = Region {
        x: 0,
        y: 0,
        width: 32,
        height: 8,
    };
    let (est, c, _, _) = cases().remove(0);
    let cpu = render_region(&image, region, &est, &c, None, None).unwrap();
    let ours = gpu
        .render_region(&image, region, &est, &c, None, None)
        .unwrap();
    for (a, b) in cpu.data().iter().zip(ours.data()) {
        assert!((a - b).abs() <= TOLERANCE, "CPU {a} GPU {b}");
    }
}

#[test]
fn a_region_too_large_for_the_device_is_an_error_not_a_panic() {
    let Some(gpu) = gpu() else { return };
    // 4096 x 2800 RGB floats is more than the 128 MiB storage binding the
    // device is opened with.
    let image = Image::new(4096, 2800);
    let region = Region {
        x: 0,
        y: 0,
        width: 4096,
        height: 2800,
    };
    let (est, c, _, _) = cases().remove(0);
    assert!(matches!(
        gpu.render_region(&image, region, &est, &c, None, None),
        Err(negg_render::gpu::GpuError::Failed(_))
    ));
}
