// SPDX-License-Identifier: Apache-2.0

//! Registration of hand-cropped reference renders onto the engine's render
//! (`negg_cli::eval::align`, docs/ADR/0028), on synthetic images only.
//!
//! The "render" is an analytic scene in linear Rec.2020 (smooth colour
//! blobs on a gradient with a fine texture), and each "reference" is the
//! same scene sampled the way another tool's export would be: cropped,
//! possibly scaled, mirrored and turned, sRGB-encoded and quantised to
//! 8 bits like a JPEG.
//!
//! The timing of a 24 MP render against a 4600x3000 reference is ignored
//! by default because it only means anything in release:
//!
//! ```sh
//! cargo test --release -p negg-cli --test eval_align -- --ignored --nocapture
//! ```

use std::time::Instant;

use negg_cli::eval::align::{self, MIN_CORRELATION, Orientation, register};
use negg_cli::eval::metrics::aligned_stats;
use negg_cms::lab::srgb_oetf;
use negg_cms::matrix::{REC709_TO_REC2020, mul_vec};
use negg_types::Image;
use rayon::prelude::*;

/// A small deterministic generator (64-bit LCG) for the scene layout.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Anything that can be sampled at continuous pixel coordinates as linear
/// Rec.709.
trait Sampled {
    fn sample(&self, x: f64, y: f64) -> [f32; 3];
}

impl Sampled for Scene {
    fn sample(&self, x: f64, y: f64) -> [f32; 3] {
        self.at(x, y)
    }
}

/// A colour scene on a `w` by `h` frame, evaluated at continuous pixel
/// coordinates, as linear Rec.709 in `[0, 1]`.
struct Scene {
    w: f64,
    h: f64,
    /// Background gradient per channel: `[base, along x, along y]`.
    gradient: [[f64; 3]; 3],
    /// Texture frequencies (radians per pixel), scaled with the frame.
    texture: [f64; 4],
    blobs: Vec<([f64; 2], f64, [f64; 3])>,
}

impl Scene {
    fn new(seed: u64, w: usize, h: usize) -> Scene {
        let mut rng = Lcg(seed);
        let (wf, hf) = (w as f64, h as f64);
        let blobs = (0..14)
            .map(|_| {
                let centre = [rng.next() * wf, rng.next() * hf];
                let radius = (0.03 + 0.10 * rng.next()) * wf;
                let colour = [
                    rng.next() * 0.6 - 0.3,
                    rng.next() * 0.6 - 0.3,
                    rng.next() * 0.6 - 0.3,
                ];
                (centre, radius, colour)
            })
            .collect();
        let gradient = std::array::from_fn(|_| {
            [
                0.1 + 0.1 * rng.next(),
                0.4 * rng.next() - 0.2,
                0.4 * rng.next() - 0.2,
            ]
        });
        // Periods of about 6 to 20 pixels on a 360 pixel wide frame, and
        // the same fraction of the frame at any other size, as the detail
        // of a photograph would be.
        let texture = std::array::from_fn(|_| (0.3 + 0.8 * rng.next()) * 360.0 / wf);
        Scene {
            w: wf,
            h: hf,
            gradient,
            texture,
            blobs,
        }
    }

    fn at(&self, x: f64, y: f64) -> [f32; 3] {
        let (u, v) = (x / self.w, y / self.h);
        let mut c = self.gradient.map(|g| 0.1 + g[0] + g[1] * u + g[2] * v);
        for (centre, radius, colour) in &self.blobs {
            let d2 = ((x - centre[0]).powi(2) + (y - centre[1]).powi(2)) / (radius * radius);
            let g = (-d2).exp();
            for k in 0..3 {
                c[k] += colour[k] * g;
            }
        }
        // A fine texture, so the finest level has detail to lock onto.
        let f = &self.texture;
        let t = 0.04 * (f[0] * x + 0.3 * f[1] * y).sin() * (f[2] * y - 0.3 * f[3] * x).sin();
        c.map(|v| (v + t).clamp(0.02, 0.95) as f32)
    }

    /// The scene as the engine's render: linear Rec.2020, one sample per
    /// pixel centre.
    fn render(&self, w: usize, h: usize) -> Image {
        let mut data = vec![0.0f32; w * h * 3];
        data.par_chunks_mut(w * 3).enumerate().for_each(|(y, row)| {
            for (x, px) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                let rgb = mul_vec(REC709_TO_REC2020, self.at(x as f64, y as f64));
                px.copy_from_slice(&rgb);
            }
        });
        Image::from_vec(w, h, data).unwrap()
    }
}

/// A reference export of `scene`: the oriented reference is `ow` by `oh`
/// pixels whose pixel `(u, v)` shows the scene at render position
/// `offset + scale * (u, v)`; it is then stored un-oriented (the inverse
/// of `orientation` applied) as sRGB quantised to 8 bits.
fn reference(
    scene: &Scene,
    ow: usize,
    oh: usize,
    scale: f64,
    offset: [f64; 2],
    orientation: Orientation,
) -> Image {
    reference_rotated(scene, ow, oh, scale, offset, 0.0, orientation)
}

/// Like [`reference`], with the oriented reference also turned clockwise
/// by `angle_degrees` about its own centre (`offset` is where its top-left
/// pixel would land without the turn).
fn reference_rotated(
    scene: &dyn Sampled,
    ow: usize,
    oh: usize,
    scale: f64,
    offset: [f64; 2],
    angle_degrees: f64,
    orientation: Orientation,
) -> Image {
    let (cu, cv) = ((ow as f64 - 1.0) / 2.0, (oh as f64 - 1.0) / 2.0);
    let (cx, cy) = (offset[0] + scale * cu, offset[1] + scale * cv);
    let (sn, c) = angle_degrees.to_radians().sin_cos();
    let mut oriented = Image::new(ow, oh);
    for v in 0..oh {
        for u in 0..ow {
            let (du, dv) = (u as f64 - cu, v as f64 - cv);
            let x = cx + scale * (c * du - sn * dv);
            let y = cy + scale * (sn * du + c * dv);
            let encoded = scene
                .sample(x, y)
                .map(|c| (srgb_oetf(c) * 255.0).round() / 255.0);
            oriented.set_pixel(u, v, encoded);
        }
    }
    // Undo the orientation: the stored file is what, turned and mirrored
    // by `orientation`, gives `oriented`. Mirroring is its own inverse and
    // the turn is undone by the opposite turn, mirror first.
    let unflip = negg_types::Geometry {
        flip_horizontal: orientation.flip_horizontal,
        ..negg_types::Geometry::default()
    };
    let unturn = negg_types::Geometry {
        rotate_degrees: (360 - orientation.rotate_degrees) % 360,
        ..negg_types::Geometry::default()
    };
    unturn.apply(&unflip.apply(&oriented).unwrap()).unwrap()
}

fn orientation(rotate_degrees: u16, flip_horizontal: bool) -> Orientation {
    Orientation {
        rotate_degrees,
        flip_horizontal,
    }
}

#[test]
fn the_reference_helper_round_trips_through_geometry() {
    // Sanity check of the test's own construction: applying the
    // orientation to the stored reference gives back the oriented image.
    let scene = Scene::new(7, 60, 40);
    let o = orientation(90, true);
    let stored = reference(&scene, 30, 20, 1.0, [5.0, 4.0], o);
    assert_eq!((stored.width(), stored.height()), (20, 30));
    let g = negg_types::Geometry {
        rotate_degrees: 90,
        flip_horizontal: true,
        ..negg_types::Geometry::default()
    };
    let back = g.apply(&stored).unwrap();
    let direct = reference(&scene, 30, 20, 1.0, [5.0, 4.0], orientation(0, false));
    assert_eq!(back, direct);
}

#[test]
fn a_cropped_mirrored_turned_8_bit_reference_is_recovered() {
    // The brief's case: an 80% crop, mirrored, turned 90 degrees and
    // quantised to 8 bits.
    let (w, h) = (360, 240);
    let scene = Scene::new(1, w, h);
    let render = scene.render(w, h);
    let (cw, ch) = (288, 192);
    let (x0, y0) = (31.0, 22.0);
    // Stored as the crop mirrored then turned 90 degrees clockwise; the
    // orientation that brings it back is a 270 degree turn then a mirror.
    let want = orientation(270, true);
    let stored = reference(&scene, cw, ch, 1.0, [x0, y0], want);
    assert_eq!((stored.width(), stored.height()), (ch, cw));

    let r = register(&render, &stored).expect("registers");
    assert_eq!(r.orientation, want, "{r:?}");
    assert!((r.scale - 1.0).abs() < 0.005, "{r:?}");
    assert!((r.offset[0] - x0).abs() < 1.0, "{r:?}");
    assert!((r.offset[1] - y0).abs() < 1.0, "{r:?}");
    assert!(r.correlation > 0.95, "{r:?}");
    assert!(r.luminance_correlation > 0.99, "{r:?}");
    assert!(r.is_aligned());
    let overlap = r.overlap_fraction(w, h);
    assert!(
        (overlap - (cw * ch) as f64 / (w * h) as f64).abs() < 0.02,
        "{overlap}"
    );

    let (de, tone) = aligned_stats(&render, &stored, &r).unwrap();
    // Only the 8-bit quantisation and sub-pixel resampling remain.
    assert!(de.mean < 0.5, "{de:?}");
    assert!(de.p95 < 1.0, "{de:?}");
    for (a, b) in [
        (tone.render.l_p1, tone.reference.l_p1),
        (tone.render.l_p50, tone.reference.l_p50),
        (tone.render.l_p99, tone.reference.l_p99),
        (tone.render.chroma_mean, tone.reference.chroma_mean),
    ] {
        assert!((a - b).abs() < 0.3, "{tone:?}");
    }
    assert!(
        tone.a_diff.abs() < 0.2 && tone.b_diff.abs() < 0.2,
        "{tone:?}"
    );
    // A real spread of tones, so the percentiles mean something.
    assert!(tone.render.l_p99 - tone.render.l_p1 > 20.0, "{tone:?}");
}

#[test]
fn a_scaled_reference_is_recovered_in_every_orientation() {
    let (w, h) = (300, 200);
    let scene = Scene::new(2, w, h);
    let render = scene.render(w, h);
    // A 75% crop exported at 70% of its size: 1.4286 render pixels per
    // reference pixel.
    let scale = 1.0 / 0.7;
    let (ow, oh) = (158, 105);
    let offset = [40.3, 27.6];
    for o in align::ORIENTATIONS {
        let stored = reference(&scene, ow, oh, scale, offset, o);
        let r = register(&render, &stored).expect("registers");
        assert_eq!(r.orientation, o, "{r:?}");
        assert!((r.scale / scale - 1.0).abs() < 0.005, "{o:?} {r:?}");
        assert!((r.offset[0] - offset[0]).abs() < 1.5, "{o:?} {r:?}");
        assert!((r.offset[1] - offset[1]).abs() < 1.5, "{o:?} {r:?}");
        assert!(r.is_aligned(), "{r:?}");
    }
}

#[test]
fn an_unrelated_reference_is_not_aligned() {
    let (w, h) = (360, 240);
    let render = Scene::new(1, w, h).render(w, h);
    // Several unrelated scenes, in both a landscape and an upright crop.
    for seed in 90..96 {
        let other = Scene::new(seed, w, h);
        for o in [orientation(0, false), orientation(90, true)] {
            let stored = reference(&other, 288, 192, 1.0, [30.0, 20.0], o);
            let r = register(&render, &stored);
            eprintln!("seed {seed} {o:?}: {:?}", r.map(|r| r.correlation));
            assert!(
                r.is_none_or(|r| !r.is_aligned()),
                "{r:?} against the threshold {MIN_CORRELATION}"
            );
        }
    }
}

#[test]
fn registration_is_deterministic() {
    let (w, h) = (200, 150);
    let scene = Scene::new(3, w, h);
    let render = scene.render(w, h);
    let stored = reference(&scene, 160, 120, 1.0, [20.0, 15.0], orientation(180, false));
    let a = register(&render, &stored).unwrap();
    let b = register(&render, &stored).unwrap();
    assert_eq!(a, b);
    let sa = aligned_stats(&render, &stored, &a).unwrap();
    let sb = aligned_stats(&render, &stored, &b).unwrap();
    assert_eq!(sa, sb);
}

/// The 24 MP timing: a 6000x4000 render against a 4600x3000 reference
/// (an 85% crop, mirrored). Prints the registration time and the time of
/// the overlap statistics separately.
#[test]
#[ignore = "timing; run in release with --ignored"]
fn registration_of_a_24_mp_render_is_quick() {
    let (w, h) = (6000, 4000);
    let scene = Scene::new(4, w, h);
    let t = Instant::now();
    let render = scene.render(w, h);
    let build_render = t.elapsed();
    let scale = 5100.0 / 4600.0;
    let offset = [450.0, 330.0];
    let want = orientation(0, true);
    let t = Instant::now();
    let stored = {
        // Built in parallel rows: the helper's per-pixel loop is slow at
        // this size.
        let (ow, oh) = (4600usize, 3000usize);
        let mut data = vec![0.0f32; ow * oh * 3];
        data.par_chunks_mut(ow * 3)
            .enumerate()
            .for_each(|(v, row)| {
                for (u, px) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                    // Mirrored: stored column u shows oriented column ow-1-u.
                    let x = offset[0] + scale * (ow - 1 - u) as f64;
                    let y = offset[1] + scale * v as f64;
                    let c = scene
                        .at(x, y)
                        .map(|c| (srgb_oetf(c) * 255.0).round() / 255.0);
                    px.copy_from_slice(&c);
                }
            });
        Image::from_vec(ow, oh, data).unwrap()
    };
    let build_reference = t.elapsed();

    let t = Instant::now();
    let r = register(&render, &stored).expect("registers");
    let register_ms = t.elapsed().as_secs_f64() * 1e3;
    let t = Instant::now();
    let (de, _) = aligned_stats(&render, &stored, &r).unwrap();
    let stats_ms = t.elapsed().as_secs_f64() * 1e3;
    println!(
        "6000x4000 render vs 4600x3000 reference: register {register_ms:.0} ms, \
         overlap statistics {stats_ms:.0} ms (scene build {:.0} ms + {:.0} ms); {r:?}; dE mean {:.3}",
        build_render.as_secs_f64() * 1e3,
        build_reference.as_secs_f64() * 1e3,
        de.mean
    );
    assert_eq!(r.orientation, want);
    assert!((r.scale / scale - 1.0).abs() < 0.002, "{r:?}");
    assert!((r.offset[0] - offset[0]).abs() < 2.0, "{r:?}");
    assert!((r.offset[1] - offset[1]).abs() < 2.0, "{r:?}");
}

#[test]
fn a_rotated_cropped_mirrored_scaled_8_bit_reference_is_recovered() {
    // Hand straightening in an editor: 1 degree on top of a crop, a scale
    // of 0.98 render pixels per reference pixel, a mirror and 8 bits.
    let (w, h) = (360, 240);
    let scene = Scene::new(11, w, h);
    let render = scene.render(w, h);
    let (ow, oh) = (300, 200);
    let scale = 0.98;
    let offset = [32.0, 21.0];
    let want = orientation(0, true);
    let stored = reference_rotated(&scene, ow, oh, scale, offset, 1.0, want);
    let r = register(&render, &stored).expect("registers");
    assert_eq!(r.orientation, want, "{r:?}");
    assert!((r.angle_degrees - 1.0).abs() < 0.1, "{r:?}");
    assert!((r.scale / scale - 1.0).abs() < 0.005, "{r:?}");
    assert!((r.offset[0] - offset[0]).abs() < 2.0, "{r:?}");
    assert!((r.offset[1] - offset[1]).abs() < 2.0, "{r:?}");
    assert!(r.is_aligned() && r.correlation > 0.9, "{r:?}");
    let (de, _) = aligned_stats(&render, &stored, &r).unwrap();
    // Only 8-bit quantisation and bilinear resampling remain.
    assert!(de.mean < 1.0, "{de:?}");
    assert!(de.p95 < 2.5, "{de:?}");
}

#[test]
fn a_rotation_either_way_and_up_to_the_limit_is_recovered() {
    let (w, h) = (360, 240);
    let scene = Scene::new(12, w, h);
    let render = scene.render(w, h);
    for angle in [-1.8, -0.6, 0.0, 0.6, 1.8] {
        let stored = reference_rotated(
            &scene,
            296,
            196,
            1.0,
            [30.0, 22.0],
            angle,
            orientation(0, false),
        );
        let r = register(&render, &stored).expect("registers");
        assert!((r.angle_degrees - angle).abs() < 0.1, "{angle}: {r:?}");
        assert!(r.is_aligned(), "{angle}: {r:?}");
    }
}

/// A dark, nearly flat frame with a few edges: a smooth diagonal ramp
/// (which every placement of a crop matches about equally in L*), a dim
/// block and a few thin bright lines, no texture.
struct DarkScene {
    w: f64,
    h: f64,
    ramp: f64,
}

impl Sampled for DarkScene {
    fn sample(&self, x: f64, y: f64) -> [f32; 3] {
        let (u, v) = (x / self.w, y / self.h);
        let base = 0.003 + self.ramp * (0.6 * u + 0.4 * v);
        let mut c = [base, base * 0.9, base * 1.1];
        if (0.58..0.83).contains(&u) && (0.55..0.85).contains(&v) {
            c = [c[0] + 0.004, c[1] + 0.004, c[2] + 0.005];
        }
        // Thin lines, 2 to 3 pixels wide on a 360 pixel frame.
        if (u - 0.27).abs() < 0.004 && (0.15..0.7).contains(&v) {
            c = [0.03, 0.03, 0.03];
        }
        if (v - 0.78).abs() < 0.006 && (0.08..0.45).contains(&u) {
            c = [0.025, 0.025, 0.025];
        }
        if (u - 0.9).abs() < 0.004 && (0.1..0.4).contains(&v) {
            c = [0.02, 0.02, 0.02];
        }
        c.map(|v| v as f32)
    }
}

impl DarkScene {
    fn render(&self, w: usize, h: usize) -> Image {
        let mut img = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let rgb = mul_vec(REC709_TO_REC2020, self.sample(x as f64, y as f64));
                img.set_pixel(x, y, rgb);
            }
        }
        img
    }
}

#[test]
fn a_dark_low_texture_scene_still_aligns() {
    // On L* alone the coarse search of this frame prefers a placement about
    // 6 pixels off at a scale of 0.96 (the faint ramp matches almost any
    // placement and the thin lines are lost in the coarse blur); the
    // gradient magnitude in the search and the several seeds per
    // orientation find the true one.
    let (w, h) = (360, 240);
    let scene = DarkScene {
        w: w as f64,
        h: h as f64,
        ramp: 0.01,
    };
    let render = scene.render(w, h);
    let want = orientation(0, false);
    for (ow, oh, offset) in [(288, 190, [36.0, 26.0]), (330, 215, [12.0, 10.0])] {
        let stored = reference_rotated(&scene, ow, oh, 1.0, offset, 0.0, want);
        let r = register(&render, &stored).expect("registers");
        assert_eq!(r.orientation, want, "{r:?}");
        assert!((r.offset[0] - offset[0]).abs() < 3.0, "{r:?}");
        assert!((r.offset[1] - offset[1]).abs() < 3.0, "{r:?}");
        assert!((r.scale - 1.0).abs() < 0.02, "{r:?}");
        assert!(r.angle_degrees.abs() < 0.3, "{r:?}");
        assert!(r.is_aligned(), "{r:?}");
    }
}

#[test]
fn a_rotated_unrelated_reference_is_not_aligned() {
    let (w, h) = (360, 240);
    let render = Scene::new(1, w, h).render(w, h);
    for seed in 90..94 {
        let other = Scene::new(seed, w, h);
        let stored = reference_rotated(
            &other,
            288,
            192,
            1.0,
            [30.0, 20.0],
            1.0,
            orientation(0, false),
        );
        let r = register(&render, &stored);
        eprintln!(
            "seed {seed}: {:?}",
            r.map(|r| (r.correlation, r.angle_degrees))
        );
        assert!(r.is_none_or(|r| !r.is_aligned()), "{r:?}");
    }
}
