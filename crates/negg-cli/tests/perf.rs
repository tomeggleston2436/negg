// SPDX-License-Identifier: Apache-2.0

//! Gate 1 of the plan: "the CLI converts a 24 MP frame in under 2 s on a
//! laptop CPU". ADR 0013 measures it end to end: decode, analysis and
//! render, export transform and write, through the same
//! [`negg_cli::convert::convert_frame`] that `negg convert` calls, on a
//! synthetic 6000x4000 linear (32-bit float) TIFF, once to 16-bit TIFF
//! and once to JPEG. A second test times the engine's analysis and
//! render alone through `negg_render::render_frame`.
//!
//! Both are ignored by default because they only mean anything in
//! release:
//!
//! ```sh
//! cargo test --release -p negg-cli --test perf -- --ignored --nocapture
//! ```

mod common;

use std::fs::File;
use std::io::BufWriter;
use std::sync::Mutex;
use std::time::Instant;

use common::TempDir;
use negg_cli::cli::Format;
use negg_cli::convert::convert_frame;
use negg_render::render_frame;
use negg_types::{Image, Settings};
use tiff::encoder::TiffEncoder;
use tiff::encoder::colortype::RGB32Float;

const WIDTH: usize = 6000;
const HEIGHT: usize = 4000;
const GATE_MS: f64 = 2000.0;

/// Held by each test for its whole run: the harness runs tests on several
/// threads, and two timings sharing the CPU would each read slow.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// A 24 MP synthetic negative: an unexposed border around a textured
/// interior, linear transmittance in `(0, 1]`. Built with cheap integer
/// arithmetic so that the construction does not dominate the test.
fn synthetic_frame() -> Image {
    let border = 300;
    let mut img = Image::new(WIDTH, HEIGHT);
    for (y, row) in img.data_mut().chunks_mut(WIDTH * 3).enumerate() {
        for (x, px) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            let interior = x >= border && x < WIDTH - border && y >= border && y < HEIGHT - border;
            if interior {
                let ramp = (x - border) as f32 / (WIDTH - 2 * border) as f32;
                let texture = ((x * 7 + y * 13) % 17) as f32 / 170.0;
                let t = (ramp * 0.85 + texture).min(1.0);
                // Density 0.25 + 1.75 t per channel, with a small colour
                // offset so the cast fit has something to do.
                *px = [
                    0.56 * (1.0 - t) + 0.01,
                    0.40 * (1.0 - t) + 0.008,
                    0.25 * (1.0 - t) + 0.006,
                ];
            } else {
                *px = [0.56, 0.40, 0.25];
            }
        }
    }
    img
}

#[test]
#[ignore = "performance gate; run in release with --ignored --nocapture"]
fn renders_a_24mp_frame_under_two_seconds() {
    let _serial = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let img = synthetic_frame();
    let matrix = [[0.9, 0.08, 0.02], [0.05, 0.9, 0.05], [0.01, 0.1, 0.89]];
    // Warm up the thread pool and page in the buffers once.
    let _ = render_frame(&img, &Settings::default(), Some(matrix)).unwrap();
    let mut times = Vec::new();
    for _ in 0..3 {
        let t = Instant::now();
        let r = render_frame(&img, &Settings::default(), Some(matrix)).unwrap();
        times.push(t.elapsed().as_secs_f64() * 1e3);
        assert_eq!(r.image.width(), WIDTH);
    }
    let best = times.iter().copied().fold(f64::INFINITY, f64::min);
    let worst = times.iter().copied().fold(0.0, f64::max);
    println!(
        "render_frame {WIDTH}x{HEIGHT}: best {best:.0} ms, worst {worst:.0} ms over {} runs \
         (gate {GATE_MS:.0} ms)",
        times.len()
    );
    assert!(
        worst < GATE_MS,
        "render took {worst:.0} ms, over the {GATE_MS:.0} ms gate"
    );
}

/// Times `convert_frame` on `input` to `format`: one warm-up run, then
/// three timed runs. Returns the best and worst wall-clock time in
/// milliseconds and prints the per-stage split of the best run.
fn time_convert(input: &std::path::Path, out: &std::path::Path, format: Format) -> (f64, f64) {
    let settings = Settings::default();
    let _ = convert_frame(input, out, &settings, format).unwrap();
    let mut best = f64::INFINITY;
    let mut worst = 0.0f64;
    let mut best_split = String::new();
    for _ in 0..3 {
        let t = Instant::now();
        let converted = convert_frame(input, out, &settings, format).unwrap();
        let ms = t.elapsed().as_secs_f64() * 1e3;
        assert_eq!(converted.render.image.width(), WIDTH);
        if ms < best {
            best = ms;
            best_split = converted.timing.to_string();
        }
        worst = worst.max(ms);
    }
    println!(
        "convert {WIDTH}x{HEIGHT} to {}: best {best:.0} ms, worst {worst:.0} ms over 3 runs \
         (gate {GATE_MS:.0} ms); best run: {best_split}",
        format.extension()
    );
    (best, worst)
}

#[test]
#[ignore = "performance gate; run in release with --ignored --nocapture"]
fn converts_a_24mp_frame_end_to_end() {
    let _serial = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let dir = TempDir::new("perf");
    let input = dir.path("frame.tif");
    {
        let img = synthetic_frame();
        let file = BufWriter::new(File::create(&input).unwrap());
        let mut enc = TiffEncoder::new(file).unwrap();
        enc.write_image::<RGB32Float>(WIDTH as u32, HEIGHT as u32, img.data())
            .unwrap();
    }
    let out = dir.path("out");
    std::fs::create_dir_all(&out).unwrap();
    let (_, tiff_worst) = time_convert(&input, &out, Format::Tiff16);
    let (_, jpeg_worst) = time_convert(&input, &out, Format::Jpeg);
    // ADR 0013: Gate 1 is the end-to-end time, to both formats.
    for (format, worst) in [("TIFF", tiff_worst), ("JPEG", jpeg_worst)] {
        assert!(
            worst < GATE_MS,
            "{format} convert took {worst:.0} ms, over the {GATE_MS:.0} ms gate"
        );
    }
}
