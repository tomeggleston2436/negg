// SPDX-License-Identifier: Apache-2.0

//! Step 7 for RAW input (ADR 0026): the print channels are read
//! as Rec.709 primaries and enter Rec.2020 with the BT.2087 matrix; the
//! camera's scene matrix is not applied. Synthetic data only: a linear
//! "camera" negative with a grey patch and a saturated blue patch, decoded
//! with a camera matrix whose off-diagonal terms are strongly negative.

mod common;

use negg_cli::convert::prepare_input;
use negg_cms::REC709_TO_REC2020;
use negg_cms::lab::{D65, linear_srgb_to_xyz, rec2020_linear_to_lab, xyz_to_lab};
use negg_cms::matrix::{camera_to_rec2020, inverse, mul_vec};
use negg_decode::{Decoded, Transfer};
use negg_render::render_frame;
use negg_types::{Image, Settings};

const W: usize = 64;
const H: usize = 48;
const GREY: (usize, usize) = (22, 24);
const BLUE: (usize, usize) = (40, 24);

/// A RAW-like decode of a synthetic negative: linear transmittance, the
/// strongly cross-coupled camera matrix, D65 as its illuminant.
fn raw_decoded() -> Decoded {
    let dmin = [0.25, 0.35, 0.5];
    let gamma = [1.6, 1.6, 1.6];
    let patch = |x: usize, y: usize| {
        let inside = |c: (usize, usize)| x >= c.0 - 3 && x < c.0 + 3 && y >= c.1 - 3 && y < c.1 + 3;
        if inside(GREY) {
            Some([0.7, 0.7, 0.7])
        } else if inside(BLUE) {
            // A saturated blue subject: the blue layer is dense, red and
            // green barely exposed.
            Some([0.05, 0.15, 1.3])
        } else {
            None
        }
    };
    let data = common::negative_linear(W, H, dmin, gamma, patch);
    // Camera RGB to XYZ with large negative off-diagonal terms; the
    // decoder records its inverse.
    let camera_to_xyz = [[1.6, -0.5, -0.1], [-0.6, 1.7, -0.1], [0.0, -0.6, 1.6]];
    let xyz_to_camera = inverse(camera_to_xyz).unwrap();
    Decoded {
        image: Image::from_vec(W, H, data).unwrap(),
        transfer: Transfer::Linear,
        xyz_to_camera: Some(xyz_to_camera),
        matrix_white: Some(D65),
        camera: Some("Synthetic".into()),
        white_level: [1.0; 3],
        black_level: [0.0; 3],
        warnings: Vec::new(),
    }
}

fn lab_hue_chroma(lab: [f32; 3]) -> (f32, f32) {
    (lab[2].atan2(lab[1]), lab[1].hypot(lab[2]))
}

#[test]
fn raw_print_output_uses_the_bt2087_matrix_not_the_camera_matrix() {
    let decoded = raw_decoded();
    let camera_matrix = camera_to_rec2020(decoded.xyz_to_camera.unwrap(), D65).unwrap();
    let prepared = prepare_input(decoded).unwrap();
    assert_eq!(prepared.camera_to_working, Some(REC709_TO_REC2020));

    let settings = Settings::default();
    // The print values before step 7, and the rendered output.
    let print = render_frame(&prepared.linear, &settings, None).unwrap();
    let out = render_frame(&prepared.linear, &settings, prepared.camera_to_working).unwrap();

    // Every pixel is exactly the BT.2087 mapping of the print values.
    for (p, o) in print
        .image
        .data()
        .as_chunks::<3>()
        .0
        .iter()
        .zip(out.image.data().as_chunks::<3>().0)
    {
        let want = mul_vec(REC709_TO_REC2020, *p);
        for c in 0..3 {
            assert!((o[c] - want[c]).abs() < 1e-5, "{o:?} vs {want:?}");
        }
    }

    // Grey stays neutral.
    let g = out.image.pixel(GREY.0, GREY.1);
    let spread =
        g.iter().cloned().fold(f32::MIN, f32::max) - g.iter().cloned().fold(f32::MAX, f32::min);
    assert!(spread < 1e-4 * g[0].max(1e-3) + 1e-5, "grey {g:?}");

    // The blue patch is blue and survives intact: no channel is clipped to
    // zero, and its hue and chroma match the print values read as Rec.709.
    let pb = print.image.pixel(BLUE.0, BLUE.1);
    let ob = out.image.pixel(BLUE.0, BLUE.1);
    assert!(
        pb[2] > 2.0 * pb[0] && pb[2] > 2.0 * pb[1],
        "not blue: {pb:?}"
    );
    assert!(ob.iter().all(|&v| v > 1e-3), "channel clipped: {ob:?}");
    let (h_in, c_in) = lab_hue_chroma(xyz_to_lab(linear_srgb_to_xyz(pb), D65));
    let (h_out, c_out) = lab_hue_chroma(rec2020_linear_to_lab(ob));
    assert!((h_in - h_out).abs() < 0.02, "hue {h_in} -> {h_out} rad");
    assert!(
        (c_in - c_out).abs() < 0.02 * c_in,
        "chroma {c_in} -> {c_out}"
    );

    // The discriminating check: the old camera matrix would have driven
    // this blue out of gamut (a negative channel, which clips and turns
    // the blue violet), the defect this test guards.
    let old = mul_vec(camera_matrix, pb);
    assert!(
        old.iter().any(|&v| v < 0.0),
        "camera matrix no longer pathological: {old:?}"
    );
}

#[test]
fn raw_without_a_camera_matrix_also_uses_bt2087() {
    let mut decoded = raw_decoded();
    decoded.xyz_to_camera = None;
    decoded.matrix_white = None;
    let prepared = prepare_input(decoded).unwrap();
    assert_eq!(prepared.camera_to_working, Some(REC709_TO_REC2020));
    assert!(prepared.matrix_description.unwrap().contains("BT.2087"));
}
