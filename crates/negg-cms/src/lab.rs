// SPDX-License-Identifier: MPL-2.0

//! sRGB, CIE XYZ, CIE 1976 L*a*b* and the CIEDE2000 colour difference.
//!
//! Pure functions on `[f32; 3]` triples. Nothing here touches an image
//! buffer or a file; callers map these over pixels themselves. The
//! CIEDE2000 computation runs in `f64` internally because the hue-angle
//! branches lose precision badly in `f32` near the axes.

/// The D65 reference white in CIE XYZ, normalised to `Y = 1`, as used by
/// the sRGB standard (IEC 61966-2-1).
pub const D65: [f32; 3] = [0.95047, 1.0, 1.08883];

/// The sRGB electro-optical transfer function (IEC 61966-2-1), mapping an
/// encoded value to linear light. Piecewise: `v / 12.92` below 0.04045,
/// otherwise `((v + 0.055) / 1.055)^2.4`. Values outside `[0, 1]` are
/// passed through the same branches without clamping.
pub fn srgb_eotf(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// The sRGB opto-electronic transfer function (IEC 61966-2-1), the inverse
/// of [`srgb_eotf`]: `12.92 * v` below 0.0031308, otherwise
/// `1.055 * v^(1/2.4) - 0.055`.
pub fn srgb_oetf(v: f32) -> f32 {
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// Linear sRGB (D65 primaries) to CIE XYZ using the IEC 61966-2-1 matrix
/// as commonly tabulated (Lindbloom's sRGB D65 matrix):
///
/// ```text
/// 0.4124564 0.3575761 0.1804375
/// 0.2126729 0.7151522 0.0721750
/// 0.0193339 0.1191920 0.9503041
/// ```
pub fn linear_srgb_to_xyz(rgb: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = rgb;
    [
        0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b,
        0.212_672_9 * r + 0.715_152_2 * g + 0.072_175_0 * b,
        0.019_333_9 * r + 0.119_192 * g + 0.950_304_1 * b,
    ]
}

/// The CIE 1976 `f(t)` companding function: the cube root above
/// `(6/29)^3`, and the linear segment `t / (3 * (6/29)^2) + 4/29` below it.
fn lab_f(t: f32) -> f32 {
    const DELTA: f32 = 6.0 / 29.0;
    const DELTA_CUBED: f32 = DELTA * DELTA * DELTA;
    if t > DELTA_CUBED {
        t.cbrt()
    } else {
        t / (3.0 * DELTA * DELTA) + 4.0 / 29.0
    }
}

/// CIE XYZ to CIE 1976 L*a*b* relative to `white` (CIE 15:2004):
/// `L* = 116 f(Y/Yn) - 16`, `a* = 500 (f(X/Xn) - f(Y/Yn))`,
/// `b* = 200 (f(Y/Yn) - f(Z/Zn))`.
pub fn xyz_to_lab(xyz: [f32; 3], white: [f32; 3]) -> [f32; 3] {
    let fx = lab_f(xyz[0] / white[0]);
    let fy = lab_f(xyz[1] / white[1]);
    let fz = lab_f(xyz[2] / white[2]);
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// Encoded (non-linear) sRGB to CIE 1976 L*a*b* under D65: the composition
/// of [`srgb_eotf`] per channel, [`linear_srgb_to_xyz`] and
/// [`xyz_to_lab`] with [`D65`].
pub fn srgb_to_lab(encoded_rgb: [f32; 3]) -> [f32; 3] {
    let linear = [
        srgb_eotf(encoded_rgb[0]),
        srgb_eotf(encoded_rgb[1]),
        srgb_eotf(encoded_rgb[2]),
    ];
    xyz_to_lab(linear_srgb_to_xyz(linear), D65)
}

/// Linear Rec.2020 (D65) to CIE 1976 L*a*b* under D65: the composition
/// of [`crate::matrix::REC2020_TO_XYZ`] and [`xyz_to_lab`] with [`D65`].
/// This is how `negg eval` scores the engine's own output, which leaves
/// step 7 of `docs/MATHS.md` in the linear Rec.2020 working space.
pub fn rec2020_linear_to_lab(rgb: [f32; 3]) -> [f32; 3] {
    xyz_to_lab(
        crate::matrix::mul_vec(*crate::matrix::REC2020_TO_XYZ, rgb),
        D65,
    )
}

/// The CIEDE2000 colour difference between two L*a*b* colours, with
/// `kL = kC = kH = 1`, following Sharma, Wu and Dalal (2005), "The CIEDE2000
/// color-difference formula: implementation notes, supplementary test data,
/// and mathematical observations", Color Research and Application 30(1).
/// The arithmetic is done in `f64`; the inputs and result are `f32` to match
/// the rest of the pipeline.
pub fn ciede2000(lab1: [f32; 3], lab2: [f32; 3]) -> f32 {
    let (l1, a1, b1) = (f64::from(lab1[0]), f64::from(lab1[1]), f64::from(lab1[2]));
    let (l2, a2, b2) = (f64::from(lab2[0]), f64::from(lab2[1]), f64::from(lab2[2]));

    // Step 1: chroma with the a* axis rescaled by G.
    let c1 = a1.hypot(b1);
    let c2 = a2.hypot(b2);
    let c_bar = (c1 + c2) / 2.0;
    let c_bar_7 = c_bar.powi(7);
    let g = 0.5 * (1.0 - (c_bar_7 / (c_bar_7 + 25f64.powi(7))).sqrt());
    let a1p = (1.0 + g) * a1;
    let a2p = (1.0 + g) * a2;
    let c1p = a1p.hypot(b1);
    let c2p = a2p.hypot(b2);
    let h1p = hue_angle(a1p, b1);
    let h2p = hue_angle(a2p, b2);

    // Step 2: the differences.
    let dlp = l2 - l1;
    let dcp = c2p - c1p;
    let dhp_angle = if c1p * c2p == 0.0 {
        0.0
    } else {
        let d = h2p - h1p;
        if d.abs() <= 180.0 {
            d
        } else if d > 180.0 {
            d - 360.0
        } else {
            d + 360.0
        }
    };
    let dhp = 2.0 * (c1p * c2p).sqrt() * (dhp_angle.to_radians() / 2.0).sin();

    // Step 3: the weighting functions and the rotation term.
    let lp_bar = (l1 + l2) / 2.0;
    let cp_bar = (c1p + c2p) / 2.0;
    let hp_bar = if c1p * c2p == 0.0 {
        h1p + h2p
    } else {
        let sum = h1p + h2p;
        if (h1p - h2p).abs() <= 180.0 {
            sum / 2.0
        } else if sum < 360.0 {
            (sum + 360.0) / 2.0
        } else {
            (sum - 360.0) / 2.0
        }
    };
    let t = 1.0 - 0.17 * (hp_bar - 30.0).to_radians().cos()
        + 0.24 * (2.0 * hp_bar).to_radians().cos()
        + 0.32 * (3.0 * hp_bar + 6.0).to_radians().cos()
        - 0.20 * (4.0 * hp_bar - 63.0).to_radians().cos();
    let d_theta = 30.0 * (-((hp_bar - 275.0) / 25.0).powi(2)).exp();
    let cp_bar_7 = cp_bar.powi(7);
    let rc = 2.0 * (cp_bar_7 / (cp_bar_7 + 25f64.powi(7))).sqrt();
    let lp_bar_sq = (lp_bar - 50.0).powi(2);
    let sl = 1.0 + 0.015 * lp_bar_sq / (20.0 + lp_bar_sq).sqrt();
    let sc = 1.0 + 0.045 * cp_bar;
    let sh = 1.0 + 0.015 * cp_bar * t;
    let rt = -(2.0 * d_theta).to_radians().sin() * rc;

    let (kl, kc, kh) = (1.0, 1.0, 1.0);
    let term_l = dlp / (kl * sl);
    let term_c = dcp / (kc * sc);
    let term_h = dhp / (kh * sh);
    let de = (term_l * term_l + term_c * term_c + term_h * term_h + rt * term_c * term_h).sqrt();
    de as f32
}

/// Hue angle in degrees in `[0, 360)`, with the convention that a point on
/// the origin (`a = b = 0`) has hue 0.
fn hue_angle(a: f64, b: f64) -> f64 {
    if a == 0.0 && b == 0.0 {
        return 0.0;
    }
    let h = b.atan2(a).to_degrees();
    if h < 0.0 { h + 360.0 } else { h }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(got: [f32; 3], want: [f32; 3], tol: f32) {
        for i in 0..3 {
            assert!(
                (got[i] - want[i]).abs() <= tol,
                "channel {i}: got {got:?}, want {want:?} within {tol}"
            );
        }
    }

    #[test]
    fn white_is_l100() {
        assert_close(srgb_to_lab([1.0, 1.0, 1.0]), [100.0, 0.0, 0.0], 0.01);
    }

    #[test]
    fn black_is_zero() {
        assert_close(srgb_to_lab([0.0, 0.0, 0.0]), [0.0, 0.0, 0.0], 1e-6);
    }

    #[test]
    fn pure_red() {
        assert_close(srgb_to_lab([1.0, 0.0, 0.0]), [53.24, 80.09, 67.20], 0.05);
    }

    #[test]
    fn rec2020_white_and_grey_are_neutral() {
        assert_close(rec2020_linear_to_lab([1.0; 3]), [100.0, 0.0, 0.0], 0.01);
        assert_close(rec2020_linear_to_lab([0.0; 3]), [0.0, 0.0, 0.0], 1e-6);
        // Linear 18% grey is L* 49.5 in both spaces, since both share D65.
        let rec = rec2020_linear_to_lab([0.18; 3]);
        let srgb = srgb_to_lab([srgb_oetf(0.18); 3]);
        assert_close(rec, srgb, 0.01);
        assert!((rec[0] - 49.5).abs() < 0.05, "{rec:?}");
        // A pure Rec.2020 red is further out than sRGB red.
        let red = rec2020_linear_to_lab([1.0, 0.0, 0.0]);
        assert!(red[1] > srgb_to_lab([1.0, 0.0, 0.0])[1], "{red:?}");
    }

    #[test]
    fn transfer_functions_round_trip() {
        for i in 0..=1000 {
            let x = i as f32 / 1000.0;
            let back = srgb_oetf(srgb_eotf(x));
            assert!((back - x).abs() <= 1e-6, "x = {x}, back = {back}");
        }
    }

    #[test]
    fn eotf_endpoints() {
        assert_eq!(srgb_eotf(0.0), 0.0);
        assert!((srgb_eotf(1.0) - 1.0).abs() < 1e-6);
        assert!((srgb_oetf(1.0) - 1.0).abs() < 1e-6);
        // 18% grey encodes to roughly 0.46.
        assert!((srgb_oetf(0.18) - 0.4613).abs() < 1e-3);
    }

    /// The 34 published test pairs (Sharma, Wu and Dalal 2005, Table 1):
    /// `(L1, a1, b1, L2, a2, b2, dE00)`.
    const SHARMA_PAIRS: [(f32, f32, f32, f32, f32, f32, f32); 34] = [
        (50.0000, 2.6772, -79.7751, 50.0000, 0.0000, -82.7485, 2.0425),
        (50.0000, 3.1571, -77.2803, 50.0000, 0.0000, -82.7485, 2.8615),
        (50.0000, 2.8361, -74.0200, 50.0000, 0.0000, -82.7485, 3.4412),
        (
            50.0000, -1.3802, -84.2814, 50.0000, 0.0000, -82.7485, 1.0000,
        ),
        (
            50.0000, -1.1848, -84.8006, 50.0000, 0.0000, -82.7485, 1.0000,
        ),
        (
            50.0000, -0.9009, -85.5211, 50.0000, 0.0000, -82.7485, 1.0000,
        ),
        (50.0000, 0.0000, 0.0000, 50.0000, -1.0000, 2.0000, 2.3669),
        (50.0000, -1.0000, 2.0000, 50.0000, 0.0000, 0.0000, 2.3669),
        (50.0000, 2.4900, -0.0010, 50.0000, -2.4900, 0.0009, 7.1792),
        (50.0000, 2.4900, -0.0010, 50.0000, -2.4900, 0.0010, 7.1792),
        (50.0000, 2.4900, -0.0010, 50.0000, -2.4900, 0.0011, 7.2195),
        (50.0000, 2.4900, -0.0010, 50.0000, -2.4900, 0.0012, 7.2195),
        (50.0000, -0.0010, 2.4900, 50.0000, 0.0009, -2.4900, 4.8045),
        (50.0000, -0.0010, 2.4900, 50.0000, 0.0010, -2.4900, 4.8045),
        (50.0000, -0.0010, 2.4900, 50.0000, 0.0011, -2.4900, 4.7461),
        (50.0000, 2.5000, 0.0000, 50.0000, 0.0000, -2.5000, 4.3065),
        (50.0000, 2.5000, 0.0000, 73.0000, 25.0000, -18.0000, 27.1492),
        (50.0000, 2.5000, 0.0000, 61.0000, -5.0000, 29.0000, 22.8977),
        (50.0000, 2.5000, 0.0000, 56.0000, -27.0000, -3.0000, 31.9030),
        (50.0000, 2.5000, 0.0000, 58.0000, 24.0000, 15.0000, 19.4535),
        (50.0000, 2.5000, 0.0000, 50.0000, 3.1736, 0.5854, 1.0000),
        (50.0000, 2.5000, 0.0000, 50.0000, 3.2972, 0.0000, 1.0000),
        (50.0000, 2.5000, 0.0000, 50.0000, 1.8634, 0.5757, 1.0000),
        (50.0000, 2.5000, 0.0000, 50.0000, 3.2592, 0.3350, 1.0000),
        (
            60.2574, -34.0099, 36.2677, 60.4626, -34.1751, 39.4387, 1.2644,
        ),
        (
            63.0109, -31.0961, -5.8663, 62.8187, -29.7946, -4.0864, 1.2630,
        ),
        (61.2901, 3.7196, -5.3901, 61.4292, 2.2480, -4.9620, 1.8731),
        (35.0831, -44.1164, 3.7933, 35.0232, -40.0716, 1.5901, 1.8645),
        (
            22.7233, 20.0904, -46.6940, 23.0331, 14.9730, -42.5619, 2.0373,
        ),
        (36.4612, 47.8580, 18.3852, 36.2715, 50.5065, 21.2231, 1.4146),
        (90.8027, -2.0831, 1.4410, 91.1528, -1.6435, 0.0447, 1.4441),
        (90.9257, -0.5406, -0.9208, 88.6381, -0.8985, -0.7239, 1.5381),
        (6.7747, -0.2908, -2.4247, 5.8714, -0.0985, -2.2286, 0.6377),
        (2.0776, 0.0795, -1.1350, 0.9033, -0.0636, -0.5514, 0.9082),
    ];

    #[test]
    fn ciede2000_matches_sharma_test_pairs() {
        for (i, &(l1, a1, b1, l2, a2, b2, want)) in SHARMA_PAIRS.iter().enumerate() {
            let got = ciede2000([l1, a1, b1], [l2, a2, b2]);
            assert!(
                (got - want).abs() <= 1e-4,
                "pair {}: got {got:.5}, want {want:.4}",
                i + 1
            );
        }
    }

    #[test]
    fn ciede2000_is_symmetric_and_zero_on_identity() {
        for &(l1, a1, b1, l2, a2, b2, _) in SHARMA_PAIRS.iter() {
            let a = [l1, a1, b1];
            let b = [l2, a2, b2];
            assert!((ciede2000(a, b) - ciede2000(b, a)).abs() <= 1e-6);
            assert_eq!(ciede2000(a, a), 0.0);
            assert_eq!(ciede2000(b, b), 0.0);
        }
    }

    #[test]
    fn hue_angle_wraps_into_full_circle() {
        assert_eq!(hue_angle(0.0, 0.0), 0.0);
        assert!((hue_angle(1.0, 0.0)).abs() < 1e-12);
        assert!((hue_angle(0.0, 1.0) - 90.0).abs() < 1e-12);
        assert!((hue_angle(-1.0, 0.0) - 180.0).abs() < 1e-12);
        assert!((hue_angle(0.0, -1.0) - 270.0).abs() < 1e-12);
    }
}
