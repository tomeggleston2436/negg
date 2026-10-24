// SPDX-License-Identifier: Apache-2.0

//! `negg convert` and `negg batch`: the conversion itself lives in
//! [`negg_session`] (Ruling 68) and is re-exported here, so that the CLI's
//! tests and callers keep their paths. This module keeps what only the CLI
//! needs: the `--dump-stats` block and the wrappers that take the clap
//! [`Format`].

use std::path::{Path, PathBuf};

use negg_types::Settings;

pub use negg_session::{
    BatchSummary, ConvertError, Converted, FrameOptions, PreparedInput, SettingsError, Timing,
    batch_inputs, check_targets, is_batch_input, parse_settings, prepare_input,
};

use crate::cli::Format;

impl From<Format> for negg_session::Format {
    fn from(format: Format) -> negg_session::Format {
        match format {
            Format::Tiff16 => negg_session::Format::Tiff16,
            Format::Jpeg => negg_session::Format::Jpeg,
        }
    }
}

/// Prints a converted frame's notes to stderr as
/// `negg convert: <name>: <note>`.
fn print_notes(input: &Path, converted: &Converted) {
    let name = input
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    for note in &converted.notes {
        eprintln!("negg convert: {name}: {note}");
    }
}

/// The output image path for `input` in `out_dir`
/// ([`negg_session::output_path`] with the clap [`Format`]).
pub fn output_path(input: &Path, out_dir: &Path, format: Format) -> PathBuf {
    negg_session::output_path(input, out_dir, format.into())
}

/// Converts one frame as phase 1 ([`negg_session::convert_frame`] with the
/// clap [`Format`]); the decoder warnings and notes go to stderr prefixed
/// with `negg convert: <name>:`.
///
/// # Errors
///
/// As [`negg_session::convert_frame`].
pub fn convert_frame(
    input: &Path,
    out_dir: &Path,
    settings: &Settings,
    format: Format,
) -> Result<Converted, ConvertError> {
    convert_frame_with(input, out_dir, settings, format, &FrameOptions::default())
}

/// [`negg_session::convert_frame_with`] with the clap [`Format`]; the
/// decoder warnings and notes go to stderr prefixed with
/// `negg convert: <name>:`.
///
/// # Errors
///
/// As [`negg_session::convert_frame_with`].
pub fn convert_frame_with(
    input: &Path,
    out_dir: &Path,
    settings: &Settings,
    format: Format,
    opts: &FrameOptions,
) -> Result<Converted, ConvertError> {
    let converted =
        negg_session::convert_frame_with(input, out_dir, settings, format.into(), opts)?;
    print_notes(input, &converted);
    Ok(converted)
}

/// The `--dump-stats` block for one frame: the estimates of steps 3 to 6,
/// whether the step 5 black-point floor raised `x0` (ADR 0030), the grid
/// and image sizes, and whether the step 7 matrix was applied.
pub fn format_stats(input: &Path, converted: &Converted) -> String {
    let r = &converted.render;
    let e = &r.estimates;
    let three = |v: [f32; 3]| format!("[{:.4}, {:.4}, {:.4}]", v[0], v[1], v[2]);
    let mut s = String::new();
    s.push_str(&format!("frame: {}\n", input.display()));
    s.push_str(&format!(
        "dimensions: {}x{}\n",
        r.image.width(),
        r.image.height()
    ));
    s.push_str(&format!("grid cells: {}\n", r.grid_cells));
    s.push_str(&format!(
        "dmin: {} ({})\n",
        three(e.dmin),
        source_name(&e.dmin_source)
    ));
    s.push_str(&format!("dmax: {}\n", three(e.dmax)));
    s.push_str(&format!(
        "gamma: {} ({})\n",
        three(e.gamma),
        source_name(&e.gamma_source)
    ));
    s.push_str(&format!("anchor: {:.4}\n", e.anchor));
    s.push_str(&format!("x0: {:.4}\n", e.x0));
    s.push_str(&format!(
        "black point floor: {}\n",
        if r.black_point_floor { "on" } else { "off" }
    ));
    s.push_str(&format!(
        "cast: slope_r {} offset_r {} slope_b {} offset_b {} cells_used {}\n",
        four(e.cast.slope_r),
        four(e.cast.offset_r),
        four(e.cast.slope_b),
        four(e.cast.offset_b),
        e.cast.cells_used
    ));
    s.push_str(&format!(
        "matrix: {}\n",
        converted.matrix_applied.unwrap_or("none")
    ));
    s
}

/// `v` to four decimals, with a value that rounds to zero printed as
/// `0.0000` rather than `-0.0000`.
pub(crate) fn four(v: f32) -> String {
    let rounded = (f64::from(v) * 1e4).round() / 1e4 + 0.0;
    format!("{rounded:.4}")
}

/// The snake_case name of a serde-renamed enum value, as the sidecar
/// writes it.
pub(crate) fn source_name<T: serde::Serialize>(source: &T) -> String {
    serde_json::to_value(source)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use negg_render::FrameRender;
    use negg_types::{CastCorrection, DminSource, Estimates, GammaSource, Image};

    #[test]
    fn four_decimals_never_print_a_negative_zero() {
        assert_eq!(four(-1e-9), "0.0000");
        assert_eq!(four(-0.0), "0.0000");
        assert_eq!(four(-0.0002), "-0.0002");
        assert_eq!(four(1.05), "1.0500");
    }

    #[test]
    fn source_names_are_snake_case() {
        assert_eq!(source_name(&DminSource::Border), "border");
        assert_eq!(source_name(&DminSource::Percentile), "percentile");
        assert_eq!(source_name(&GammaSource::Range), "range");
        assert_eq!(source_name(&GammaSource::Fixed), "fixed");
    }

    #[test]
    fn stats_block_names_every_estimate() {
        let converted = Converted {
            output: PathBuf::from("out/f.negg.tif"),
            sidecar: PathBuf::from("f.negg.json"),
            render: FrameRender {
                image: Image::new(4, 3),
                estimates: Estimates {
                    dmin: [0.2, 0.5, 0.7],
                    dmin_source: DminSource::Border,
                    dmax: [1.9, 2.2, 2.4],
                    gamma: [1.7, 1.7, 1.7],
                    gamma_source: GammaSource::Range,
                    neutral_offset: [0.0; 3],
                    anchor: -0.42,
                    x0: 1.25,
                    cast: CastCorrection {
                        slope_r: 1.05,
                        offset_r: -0.01,
                        slope_b: 0.97,
                        offset_b: 0.02,
                        cells_used: 123,
                    },
                    cast_source: negg_types::CastSource::Fit,
                },
                grid_cells: 12,
                black_point_floor: true,
            },
            matrix_applied: Some("Rec.709 print primaries to Rec.2020 applied (BT.2087)"),
            timing: Timing::default(),
            notes: Vec::new(),
        };
        let s = format_stats(Path::new("f.dng"), &converted);
        assert!(s.contains("frame: f.dng\n"), "{s}");
        assert!(s.contains("dimensions: 4x3\n"), "{s}");
        assert!(s.contains("grid cells: 12\n"), "{s}");
        assert!(
            s.contains("dmin: [0.2000, 0.5000, 0.7000] (border)\n"),
            "{s}"
        );
        assert!(s.contains("dmax: [1.9000, 2.2000, 2.4000]\n"), "{s}");
        assert!(
            s.contains("gamma: [1.7000, 1.7000, 1.7000] (range)\n"),
            "{s}"
        );
        assert!(s.contains("anchor: -0.4200\n"), "{s}");
        assert!(s.contains("x0: 1.2500\n"), "{s}");
        assert!(s.contains("black point floor: on\n"), "{s}");
        assert!(
            s.contains(
                "cast: slope_r 1.0500 offset_r -0.0100 slope_b 0.9700 offset_b 0.0200 cells_used 123\n"
            ),
            "{s}"
        );
        assert!(
            s.contains("matrix: Rec.709 print primaries to Rec.2020 applied (BT.2087)\n"),
            "{s}"
        );
        assert!(!s.contains('\u{2014}') && !s.contains('\u{2013}'));
    }

    #[test]
    fn the_clap_format_maps_to_the_session_format() {
        assert_eq!(
            negg_session::Format::from(Format::Tiff16),
            negg_session::Format::Tiff16
        );
        assert_eq!(
            negg_session::Format::from(Format::Jpeg),
            negg_session::Format::Jpeg
        );
        assert_eq!(
            output_path(Path::new("roll/a.tif"), Path::new("out"), Format::Jpeg),
            Path::new("out/a.negg.jpg")
        );
    }
}
