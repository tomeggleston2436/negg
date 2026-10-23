// SPDX-License-Identifier: MPL-2.0

//! Synthetic negatives with a known film base, pushed through the forward
//! model `D_c = Dmin_c + gamma_c * scene_density` of steps 3 and 4 of
//! `docs/MATHS.md`, to check that the estimators recover what was put in
//! and that dust does not move them (ground rule 5).

use negg_film::{analysis_grid, estimate_dmax, estimate_dmin, no_profile_gamma};
use negg_types::{DminSource, Image};

const WIDTH: usize = 400;
const HEIGHT: usize = 300;
/// Grid long side for these tests: 400 px / 100 gives 4 x 4 blocks, so a
/// block has 16 pixels and the texture measure means something. (At the
/// production value of 1024 a 400 px frame is its own grid and every cell
/// is trivially flat.)
const GRID_LONG_SIDE: usize = 100;

const DMIN: [f32; 3] = [0.22, 0.58, 0.81];
const GAMMA: [f32; 3] = [1.55, 1.45, 1.35];
/// Unexposed border width as a fraction of each dimension, on every edge.
const BORDER: f64 = 0.12;

/// A tiny deterministic generator (xorshift64*) so the tests need no
/// dependency and give the same frame every run.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed.max(1))
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in [0, 1).
    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Describes the synthetic frame.
#[derive(Clone, Copy)]
struct Scene {
    /// Whether an unexposed border of width `BORDER` at exactly Dmin wraps
    /// the content.
    border: bool,
    /// Scene density of a flat band across the top 20% of the content, if
    /// any (a featureless sky).
    sky: Option<f64>,
    /// Fraction of pixels replaced by dust at density 3.0.
    dust: f64,
    seed: u64,
}

/// Scene density at content pixel (x, y): a horizontal gradient from
/// -0.03 to 1.0 across the content width plus uniform texture of +/- 0.1,
/// clamped at 0. The negative start of the gradient means the leftmost
/// columns have a block median at exactly 0 while still being textured,
/// which is what the percentile path needs to see the true base without a
/// border.
fn scene_density(x: usize, x0: usize, x1: usize, rng: &mut Rng) -> f64 {
    let t = (x - x0) as f64 / (x1 - x0 - 1) as f64;
    let gradient = -0.03 + 1.03 * t;
    let texture = (rng.unit() - 0.5) * 0.2;
    (gradient + texture).max(0.0)
}

/// Builds the linear camera RGB image of the scene through the forward
/// model: `I_c = 10^-(Dmin_c + gamma_c * s)`.
fn negative(scene: Scene) -> Image {
    let mut rng = Rng::new(scene.seed);
    let mut img = Image::new(WIDTH, HEIGHT);
    let (bx, by) = if scene.border {
        (
            (BORDER * WIDTH as f64).round() as usize,
            (BORDER * HEIGHT as f64).round() as usize,
        )
    } else {
        (0, 0)
    };
    let (x0, x1, y0, y1) = (bx, WIDTH - bx, by, HEIGHT - by);
    let sky_rows = y0 + (y1 - y0) / 5;

    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let in_border = x < x0 || x >= x1 || y < y0 || y >= y1;
            let s = if in_border {
                0.0
            } else if let (Some(sky), true) = (scene.sky, y < sky_rows) {
                sky
            } else {
                scene_density(x, x0, x1, &mut rng)
            };
            let mut px = [0.0f32; 3];
            for c in 0..3 {
                let d = f64::from(DMIN[c]) + f64::from(GAMMA[c]) * s;
                px[c] = 10f64.powf(-d) as f32;
            }
            img.set_pixel(x, y, px);
        }
    }

    if scene.dust > 0.0 {
        let mut dust_rng = Rng::new(scene.seed ^ 0xD05F);
        let dust_value = 10f32.powf(-3.0);
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                if dust_rng.unit() < scene.dust {
                    img.set_pixel(x, y, [dust_value; 3]);
                }
            }
        }
    }
    img
}

fn assert_close(got: [f32; 3], want: [f32; 3], tol: f32, what: &str) {
    for c in 0..3 {
        assert!(
            (got[c] - want[c]).abs() < tol,
            "{what}: channel {c} got {} want {} (tolerance {tol})",
            got[c],
            want[c]
        );
    }
}

const CLEAN_WITH_BORDER: Scene = Scene {
    border: true,
    sky: None,
    dust: 0.0,
    seed: 1,
};

const CLEAN_NO_BORDER: Scene = Scene {
    border: false,
    sky: None,
    dust: 0.0,
    seed: 1,
};

#[test]
fn border_path_recovers_dmin_within_0_01() {
    let grid = analysis_grid(&negative(CLEAN_WITH_BORDER), GRID_LONG_SIDE);
    assert_eq!(grid.block, 4);
    assert_eq!((grid.width(), grid.height()), (100, 75));
    let est = estimate_dmin(&grid);
    assert_eq!(est.source, DminSource::Border);
    assert!(est.flat_border_cells >= 20, "{}", est.flat_border_cells);
    assert_close(est.value, DMIN, 0.01, "border Dmin");
}

#[test]
fn percentile_path_recovers_dmin_within_0_01_when_border_is_absent() {
    let grid = analysis_grid(&negative(CLEAN_NO_BORDER), GRID_LONG_SIDE);
    let est = estimate_dmin(&grid);
    assert_eq!(est.source, DminSource::Percentile);
    assert_close(est.value, DMIN, 0.01, "percentile Dmin");
}

#[test]
fn dmax_and_gamma_follow_the_forward_model() {
    // Scene density tops out near 1.0 (+ texture up to 0.1, but the block
    // median smooths most of that), so Dmax is about Dmin + gamma and the
    // no-profile gamma is about the true gamma.
    let grid = analysis_grid(&negative(CLEAN_NO_BORDER), GRID_LONG_SIDE);
    let dmin = estimate_dmin(&grid).value;
    let dmax = estimate_dmax(&grid);
    let gamma = no_profile_gamma(dmin, dmax);
    for c in 0..3 {
        assert!(dmax[c] > dmin[c] + 1.0, "{dmax:?} {dmin:?}");
        assert!(
            (gamma[c] - GAMMA[c]).abs() < 0.12,
            "gamma {gamma:?} vs {GAMMA:?}"
        );
    }
}

#[test]
fn two_percent_dust_moves_dmin_under_0_005_and_dmax_under_0_02() {
    for base in [CLEAN_WITH_BORDER, CLEAN_NO_BORDER] {
        let dusty = Scene { dust: 0.02, ..base };
        let clean_grid = analysis_grid(&negative(base), GRID_LONG_SIDE);
        let dusty_grid = analysis_grid(&negative(dusty), GRID_LONG_SIDE);

        let clean_dmin = estimate_dmin(&clean_grid);
        let dusty_dmin = estimate_dmin(&dusty_grid);
        assert_eq!(clean_dmin.source, dusty_dmin.source);
        assert_close(
            dusty_dmin.value,
            clean_dmin.value,
            0.005,
            "Dmin shift from dust",
        );
        assert_close(dusty_dmin.value, DMIN, 0.01, "dusty Dmin");

        let clean_dmax = estimate_dmax(&clean_grid);
        let dusty_dmax = estimate_dmax(&dusty_grid);
        assert_close(dusty_dmax, clean_dmax, 0.02, "Dmax shift from dust");
    }
}

#[test]
fn flat_sky_does_not_trigger_the_border_path() {
    // A featureless sky at scene density 0.15 across the top fifth of the
    // frame: flat, and in the border band, but 0.2 log-D denser than the
    // content minimum.
    let scene = Scene {
        border: false,
        sky: Some(0.15),
        dust: 0.0,
        seed: 1,
    };
    let grid = analysis_grid(&negative(scene), GRID_LONG_SIDE);
    let est = estimate_dmin(&grid);
    assert!(
        est.flat_border_cells >= 20,
        "the sky should have been seen as flat: {}",
        est.flat_border_cells
    );
    assert_eq!(est.source, DminSource::Percentile);
    assert_close(est.value, DMIN, 0.01, "Dmin with a flat sky");
}

#[test]
fn production_grid_of_a_small_frame_is_the_frame_itself() {
    // At the production long side a 400 px frame has block 1; the border
    // path then rests on rule 2 alone (every cell is trivially flat), and
    // it still recovers the base from the border.
    let grid = analysis_grid(&negative(CLEAN_WITH_BORDER), 1024);
    assert_eq!(grid.block, 1);
    let est = estimate_dmin(&grid);
    assert_eq!(est.source, DminSource::Border);
    assert_close(est.value, DMIN, 0.01, "block-1 border Dmin");
}
