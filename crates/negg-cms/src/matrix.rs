// SPDX-License-Identifier: MPL-2.0

//! Pure 3x3 matrix maths for step 7 of `docs/MATHS.md`: products,
//! inverses, the Bradford chromatic adaptation, RGB-to-XYZ matrices from
//! chromaticities, and the Rec.2020 working-space matrices.
//!
//! The public API is `f32` to match the rest of the pipeline; every
//! computation runs in `f64` internally and is rounded once on the way
//! out, so chained products do not accumulate single-precision error.

use std::sync::LazyLock;

use crate::error::CmsError;

/// A row-major 3x3 matrix: `m[row][column]`. A colour `v` transforms as
/// `m * v`, see [`mul_vec`].
pub type Matrix3 = [[f32; 3]; 3];

type M64 = [[f64; 3]; 3];

/// The 3x3 identity.
pub const IDENTITY: Matrix3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// The D65 reference white in CIE XYZ (`Y = 1`), the tabulated value from
/// IEC 61966-2-1 that [`crate::lab::D65`] also uses. The chromaticity
/// (0.3127, 0.3290) that ITU-R BT.2020 and the ICC specification quote
/// gives `[0.950456, 1, 1.089058]`; the two differ at the fourth decimal
/// and this crate uses the tabulated value throughout so that
/// [`crate::lab`] and this module agree exactly.
pub const D65_XYZ: [f32; 3] = crate::lab::D65;

/// The D50 reference white in CIE XYZ (`Y = 1`), the CIE 15:2004 tabulated
/// value (ASTM E308 2-degree observer). The ICC profile connection space
/// white is the rounded `[0.9642, 1, 0.8249]`.
pub const D50_XYZ: [f32; 3] = [0.96422, 1.0, 0.82521];

/// Rec.2020 red primary as CIE xy, ITU-R BT.2020 table 3.
pub const REC2020_RED: [f32; 2] = [0.708, 0.292];
/// Rec.2020 green primary as CIE xy, ITU-R BT.2020 table 3.
pub const REC2020_GREEN: [f32; 2] = [0.170, 0.797];
/// Rec.2020 blue primary as CIE xy, ITU-R BT.2020 table 3.
pub const REC2020_BLUE: [f32; 2] = [0.131, 0.046];

/// Linear Rec.709 (sRGB primaries, D65) to linear Rec.2020 (D65): the
/// matrix of ITU-R BT.2087-0, "Colour conversion from Recommendation
/// ITU-R BT.709 to Recommendation ITU-R BT.2020", equation (2), rounded
/// to four places as published.
///
/// Derivation: `XYZ_TO_REC2020 * REC709_TO_XYZ`, where `REC709_TO_XYZ`
/// is the RGB-to-XYZ matrix of the BT.709 primaries (R 0.640, 0.330;
/// G 0.300, 0.600; B 0.150, 0.060) with the D65 white, and
/// `XYZ_TO_REC2020` is the inverse of the same construction for the
/// BT.2020 primaries ([`REC2020_RED`] and friends). Both spaces share the
/// D65 white, so no chromatic adaptation is involved and every row sums to
/// 1: Rec.709 `(1, 1, 1)` maps to Rec.2020 `(1, 1, 1)`.
///
/// The values are standard, not measured (ground rule 6). Step 7 applies
/// this matrix to the print output of a RAW input, treating the three
/// print channels as Rec.709 primaries (ADR 0026, Ruling 96).
pub const REC709_TO_REC2020: Matrix3 = [
    [0.6274, 0.3293, 0.0433],
    [0.0691, 0.9195, 0.0114],
    [0.0164, 0.0880, 0.8956],
];

/// The Bradford cone response matrix (Lam 1985, as reproduced in ICC.1
/// annex E), mapping CIE XYZ to the sharpened RGB cone space in which the
/// von Kries scaling of [`bradford_adaptation`] is applied.
pub const BRADFORD: Matrix3 = [
    [0.8951, 0.2664, -0.1614],
    [-0.7502, 1.7135, 0.0367],
    [0.0389, -0.0685, 1.0296],
];

/// Linear Rec.2020 (D65) to CIE XYZ, derived from [`REC2020_RED`],
/// [`REC2020_GREEN`], [`REC2020_BLUE`] and [`D65_XYZ`] on first use. The
/// first row is `[0.6370, 0.1446, 0.1689]` to four places, matching the
/// matrix published with ITU-R BT.2020.
pub static REC2020_TO_XYZ: LazyLock<Matrix3> = LazyLock::new(|| to_f32(rec2020_to_xyz_64()));

/// CIE XYZ to linear Rec.2020 (D65): the inverse of [`REC2020_TO_XYZ`],
/// computed in `f64` from the same primaries. This is the last matrix in
/// the step 7 entry `camera RGB -> XYZ -> Rec.2020`.
pub static XYZ_TO_REC2020: LazyLock<Matrix3> = LazyLock::new(|| {
    to_f32(inverse_64(rec2020_to_xyz_64()).expect("the Rec.2020 primaries are not collinear"))
});

/// The Rec.2020-to-XYZ matrix in `f64`, shared by both lazy statics so the
/// inverse is taken before any rounding to `f32`.
fn rec2020_to_xyz_64() -> M64 {
    rgb_to_xyz_64(
        f64::from(REC2020_RED[0]),
        f64::from(REC2020_RED[1]),
        f64::from(REC2020_GREEN[0]),
        f64::from(REC2020_GREEN[1]),
        f64::from(REC2020_BLUE[0]),
        f64::from(REC2020_BLUE[1]),
        vec_to_f64(D65_XYZ),
    )
    .expect("the Rec.2020 primaries are not collinear")
}

fn to_f64(m: Matrix3) -> M64 {
    m.map(|row| row.map(f64::from))
}

fn to_f32(m: M64) -> Matrix3 {
    m.map(|row| row.map(|v| v as f32))
}

fn vec_to_f64(v: [f32; 3]) -> [f64; 3] {
    v.map(f64::from)
}

fn mul_64(a: M64, b: M64) -> M64 {
    let mut r = [[0.0; 3]; 3];
    for (i, row) in r.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    r
}

fn mul_vec_64(m: M64, v: [f64; 3]) -> [f64; 3] {
    m.map(|row| row[0] * v[0] + row[1] * v[1] + row[2] * v[2])
}

fn transpose_64(m: M64) -> M64 {
    let mut r = [[0.0; 3]; 3];
    for (i, row) in m.iter().enumerate() {
        for (j, &v) in row.iter().enumerate() {
            r[j][i] = v;
        }
    }
    r
}

/// Inverse by cofactors. `None` when the determinant is zero relative to
/// the size of the entries, so a matrix of all zeros, a repeated row, or
/// collinear rows all count as singular even when rounding leaves a
/// determinant of `1e-17` rather than exactly zero.
fn inverse_64(m: M64) -> Option<M64> {
    let [[a, b, c], [d, e, f], [g, h, i]] = m;
    let cofactors = [
        [e * i - f * h, -(d * i - f * g), d * h - e * g],
        [-(b * i - c * h), a * i - c * g, -(a * h - b * g)],
        [b * f - c * e, -(a * f - c * d), a * e - b * d],
    ];
    let det = a * cofactors[0][0] + b * cofactors[0][1] + c * cofactors[0][2];
    let scale = m.iter().flatten().fold(0.0_f64, |acc, v| acc.max(v.abs()));
    if scale == 0.0 || det.abs() <= 1e-12 * scale * scale * scale {
        return None;
    }
    let adjugate = transpose_64(cofactors);
    Some(adjugate.map(|row| row.map(|v| v / det)))
}

/// The standard derivation of an RGB-to-XYZ matrix from chromaticities:
/// with `P` the matrix whose columns are the primaries as XYZ with `Y = 1`,
/// the scaling `S = P^-1 * white` makes RGB `(1, 1, 1)` land on the white,
/// and the result is `P` with its columns scaled by `S`.
#[allow(clippy::too_many_arguments)]
fn rgb_to_xyz_64(
    rx: f64,
    ry: f64,
    gx: f64,
    gy: f64,
    bx: f64,
    by: f64,
    white: [f64; 3],
) -> Option<M64> {
    if ry <= 0.0 || gy <= 0.0 || by <= 0.0 {
        return None;
    }
    let p = [
        [rx / ry, gx / gy, bx / by],
        [1.0, 1.0, 1.0],
        [
            (1.0 - rx - ry) / ry,
            (1.0 - gx - gy) / gy,
            (1.0 - bx - by) / by,
        ],
    ];
    let s = mul_vec_64(inverse_64(p)?, white);
    Some(p.map(|row| [row[0] * s[0], row[1] * s[1], row[2] * s[2]]))
}

/// The Bradford adaptation in `f64`: `B^-1 * diag(dst / src) * B` where
/// `src` and `dst` are the two whites in Bradford cone space.
fn bradford_64(src_white: [f64; 3], dst_white: [f64; 3]) -> M64 {
    let b = to_f64(BRADFORD);
    let b_inv = inverse_64(b).expect("the Bradford matrix is invertible");
    let src = mul_vec_64(b, src_white);
    let dst = mul_vec_64(b, dst_white);
    let scale = [
        [dst[0] / src[0], 0.0, 0.0],
        [0.0, dst[1] / src[1], 0.0],
        [0.0, 0.0, dst[2] / src[2]],
    ];
    mul_64(mul_64(b_inv, scale), b)
}

/// The matrix product `a * b`, computed in `f64`.
pub fn mul(a: Matrix3, b: Matrix3) -> Matrix3 {
    to_f32(mul_64(to_f64(a), to_f64(b)))
}

/// The matrix-vector product `m * v`, computed in `f64`.
pub fn mul_vec(m: Matrix3, v: [f32; 3]) -> [f32; 3] {
    mul_vec_64(to_f64(m), vec_to_f64(v)).map(|x| x as f32)
}

/// The transpose of `m`.
pub fn transpose(m: Matrix3) -> Matrix3 {
    to_f32(transpose_64(to_f64(m)))
}

/// The inverse of `m`, computed in `f64` by cofactors.
///
/// # Errors
///
/// [`CmsError::Singular`] when the determinant is zero relative to the
/// size of the entries.
pub fn inverse(m: Matrix3) -> Result<Matrix3, CmsError> {
    inverse_64(to_f64(m)).map(to_f32).ok_or(CmsError::Singular)
}

/// The Bradford chromatic adaptation matrix taking XYZ under the
/// illuminant `src_white_xyz` to XYZ under `dst_white_xyz` (step 7 of
/// `docs/MATHS.md`, "Bradford-adapted to D65"). Both whites are CIE XYZ
/// with `Y = 1` and must be valid whites (all three Bradford cone
/// responses positive), which every real illuminant satisfies.
///
/// Adapting from D50 ([`D50_XYZ`]) to D65 ([`D65_XYZ`]) gives the familiar
/// matrix whose first row is `[0.9555766, -0.0230393, 0.0631636]`.
pub fn bradford_adaptation(src_white_xyz: [f32; 3], dst_white_xyz: [f32; 3]) -> Matrix3 {
    to_f32(bradford_64(
        vec_to_f64(src_white_xyz),
        vec_to_f64(dst_white_xyz),
    ))
}

/// The matrix taking linear RGB with the given primaries (CIE xy) and
/// white (CIE XYZ, `Y = 1`) to CIE XYZ, so that RGB `(1, 1, 1)` maps
/// exactly to `white_xyz`.
///
/// # Errors
///
/// [`CmsError::Singular`] when a primary has `y <= 0` or the three
/// primaries are collinear, since no such matrix exists.
#[allow(clippy::too_many_arguments)]
pub fn rgb_to_xyz_from_primaries(
    rx: f32,
    ry: f32,
    gx: f32,
    gy: f32,
    bx: f32,
    by: f32,
    white_xyz: [f32; 3],
) -> Result<Matrix3, CmsError> {
    rgb_to_xyz_64(
        f64::from(rx),
        f64::from(ry),
        f64::from(gx),
        f64::from(gy),
        f64::from(bx),
        f64::from(by),
        vec_to_f64(white_xyz),
    )
    .map(to_f32)
    .ok_or(CmsError::Singular)
}

/// The D50 illuminant of the ICC profile connection space, CIE XYZ as
/// ICC.1 clause 7.2.16 fixes it (`[0.9642, 1, 0.8249]`). The colorant
/// tags of a matrix/TRC profile sum to this white.
pub const ICC_PCS_D50: [f32; 3] = [0.9642, 1.0, 0.8249];

/// The step 7 entry matrix of `docs/MATHS.md` for a TIFF linearised in
/// its own ICC primaries (ADR 0012, Ruling 29): linear RGB in the
/// profile's primaries to linear Rec.2020 (D65),
///
/// ```text
/// XYZ_TO_REC2020 * bradford(ICC_PCS_D50 -> D65) * C
/// ```
///
/// where `C` has the profile's red, green and blue colorant tags (CIE XYZ
/// relative to the D50 profile connection space) as its columns. The
/// colorants sum to the PCS white, so RGB `(1, 1, 1)` lands on Rec.2020
/// `(1, 1, 1)` and the matrix is white-normalised by construction.
pub fn icc_colorants_to_rec2020(red: [f64; 3], green: [f64; 3], blue: [f64; 3]) -> Matrix3 {
    let c = [
        [red[0], green[0], blue[0]],
        [red[1], green[1], blue[1]],
        [red[2], green[2], blue[2]],
    ];
    let adapt = bradford_64(vec_to_f64(ICC_PCS_D50), vec_to_f64(D65_XYZ));
    let xyz_to_rec2020 = inverse_64(rec2020_to_xyz_64()).expect("Rec.2020 is invertible");
    to_f32(mul_64(mul_64(xyz_to_rec2020, adapt), c))
}

/// The matrix a Little-CMS matrix/TRC transform applies between the
/// linearised channels of two RGB profiles, from their colorant tags (the
/// columns of each profile's RGB-to-PCS matrix): `inverse(dst) * src`.
///
/// # Errors
///
/// [`CmsError::Singular`] when the destination colorants are singular.
pub fn colorants_to_colorants(src: [[f64; 3]; 3], dst: [[f64; 3]; 3]) -> Result<Matrix3, CmsError> {
    let columns = |c: [[f64; 3]; 3]| transpose_64(c);
    let inv = inverse_64(columns(dst)).ok_or(CmsError::Singular)?;
    Ok(to_f32(mul_64(inv, columns(src))))
}

/// The step 7 entry matrix of `docs/MATHS.md`: camera RGB to linear
/// Rec.2020 (D65), to be applied **after** inversion to `out_c`,
/// white-normalised per ADR 0012 (Ruling 28).
///
/// `xyz_to_camera` (`X2C`) is the camera's colour matrix as recorded in
/// step 1 (rawler's matrix, DNG `ColorMatrix2`, maps XYZ to camera RGB),
/// and `camera_matrix_white` (`W`) is the CIE XYZ of the illuminant that
/// matrix was measured under, `Decoded::matrix_white` (D65 for
/// `ColorMatrix2`). The result is
///
/// ```text
/// XYZ_TO_REC2020 * bradford(W -> D65) * inverse(X2C) * diag(X2C * W)
/// ```
///
/// After inversion and step 6, a neutral is equal in every camera channel,
/// so camera `(1, 1, 1)` must mean "the illuminant's white". `diag(X2C * W)`
/// scales camera `(1, 1, 1)` to the camera's response to `W`, which
/// `inverse(X2C)` takes to `W` itself; Bradford then adapts it to D65 and
/// `XYZ_TO_REC2020` lands it on Rec.2020 `(1, 1, 1)`. A camera neutral
/// therefore stays neutral whatever the matrix.
///
/// # Errors
///
/// [`CmsError::Singular`] when `xyz_to_camera` has no inverse or when the
/// camera's response to `W` is not positive in every channel (so
/// `diag(X2C * W)` cannot carry a neutral).
pub fn camera_to_rec2020(
    xyz_to_camera: Matrix3,
    camera_matrix_white: [f32; 3],
) -> Result<Matrix3, CmsError> {
    let x2c = to_f64(xyz_to_camera);
    let white = vec_to_f64(camera_matrix_white);
    let camera_to_xyz = inverse_64(x2c).ok_or(CmsError::Singular)?;
    // A real camera responds positively to white in every channel; a
    // response that is not positive (or is lost in rounding next to the
    // others) cannot carry a neutral.
    let neutral = mul_vec_64(x2c, white);
    let largest = neutral.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    if neutral
        .iter()
        .any(|v| !v.is_finite() || *v <= 1e-6 * largest)
    {
        return Err(CmsError::Singular);
    }
    let scale = [
        [neutral[0], 0.0, 0.0],
        [0.0, neutral[1], 0.0],
        [0.0, 0.0, neutral[2]],
    ];
    let adapt = bradford_64(white, vec_to_f64(D65_XYZ));
    let xyz_to_rec2020 = inverse_64(rec2020_to_xyz_64()).expect("Rec.2020 is invertible");
    Ok(to_f32(mul_64(
        mul_64(mul_64(xyz_to_rec2020, adapt), camera_to_xyz),
        scale,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_matrix_close(got: Matrix3, want: Matrix3, tol: f32) {
        for i in 0..3 {
            for j in 0..3 {
                assert!(
                    (got[i][j] - want[i][j]).abs() <= tol,
                    "[{i}][{j}]: got {got:?}, want {want:?} within {tol}"
                );
            }
        }
    }

    fn assert_vec_close(got: [f32; 3], want: [f32; 3], tol: f32) {
        for i in 0..3 {
            assert!(
                (got[i] - want[i]).abs() <= tol,
                "[{i}]: got {got:?}, want {want:?} within {tol}"
            );
        }
    }

    /// Lindbloom's sRGB (D65) to XYZ matrix, also used by `lab.rs`.
    const SRGB_TO_XYZ: Matrix3 = [
        [0.412_456_4, 0.357_576_1, 0.180_437_5],
        [0.212_672_9, 0.715_152_2, 0.072_175_0],
        [0.019_333_9, 0.119_192, 0.950_304_1],
    ];

    #[test]
    fn rec709_to_rec2020_is_the_bt2087_matrix() {
        for row in REC709_TO_REC2020 {
            assert!((row.iter().sum::<f32>() - 1.0).abs() < 1e-4, "{row:?}");
        }
        assert_vec_close(mul_vec(REC709_TO_REC2020, [1.0; 3]), [1.0; 3], 1e-4);
        // It is the product of the two primaries-derived matrices.
        let to_xyz =
            rgb_to_xyz_from_primaries(0.64, 0.33, 0.30, 0.60, 0.15, 0.06, D65_XYZ).unwrap();
        let derived = mul(*XYZ_TO_REC2020, to_xyz);
        assert_matrix_close(REC709_TO_REC2020, derived, 2e-4);
        // A saturated Rec.709 blue stays inside Rec.2020 (no negatives).
        let blue = mul_vec(REC709_TO_REC2020, [0.0, 0.0, 1.0]);
        assert!(blue.iter().all(|&v| v >= 0.0), "{blue:?}");
    }

    #[test]
    fn mul_by_identity_is_identity() {
        assert_matrix_close(mul(SRGB_TO_XYZ, IDENTITY), SRGB_TO_XYZ, 0.0);
        assert_matrix_close(mul(IDENTITY, SRGB_TO_XYZ), SRGB_TO_XYZ, 0.0);
    }

    #[test]
    fn mul_matches_hand_product() {
        let a = [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0]];
        let b = [[9.0, 8.0, 7.0], [6.0, 5.0, 4.0], [3.0, 2.0, 1.0]];
        let want = [[30.0, 24.0, 18.0], [84.0, 69.0, 54.0], [138.0, 114.0, 90.0]];
        assert_matrix_close(mul(a, b), want, 0.0);
        assert_vec_close(mul_vec(a, [1.0, 0.0, -1.0]), [-2.0, -2.0, -2.0], 0.0);
    }

    #[test]
    fn transpose_swaps_rows_and_columns() {
        let a = [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0]];
        let want = [[1.0, 4.0, 7.0], [2.0, 5.0, 8.0], [3.0, 6.0, 9.0]];
        assert_matrix_close(transpose(a), want, 0.0);
        assert_matrix_close(transpose(transpose(a)), a, 0.0);
    }

    #[test]
    fn inverse_times_matrix_is_identity() {
        let inv = inverse(SRGB_TO_XYZ).unwrap();
        assert_matrix_close(mul(inv, SRGB_TO_XYZ), IDENTITY, 1e-6);
        assert_matrix_close(mul(SRGB_TO_XYZ, inv), IDENTITY, 1e-6);
        // Lindbloom's XYZ to sRGB matrix.
        let want = [
            [3.240_454_2, -1.537_138_5, -0.498_531_4],
            [-0.969_266, 1.876_010_8, 0.041_556],
            [0.055_643_4, -0.204_025_9, 1.057_225_2],
        ];
        assert_matrix_close(inv, want, 1e-5);
    }

    #[test]
    fn singular_matrices_are_rejected() {
        assert_eq!(inverse([[0.0; 3]; 3]), Err(CmsError::Singular));
        let repeated_row = [[1.0, 2.0, 3.0], [1.0, 2.0, 3.0], [0.0, 1.0, 0.0]];
        assert_eq!(inverse(repeated_row), Err(CmsError::Singular));
        // Rows in arithmetic progression are collinear; in floating point
        // the determinant comes out as a rounding residue, not zero.
        let collinear = [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0]];
        assert_eq!(inverse(collinear), Err(CmsError::Singular));
        // A merely small but well-conditioned matrix is fine.
        let tiny = [[1e-6, 0.0, 0.0], [0.0, 1e-6, 0.0], [0.0, 0.0, 1e-6]];
        assert!(inverse(tiny).is_ok());
    }

    #[test]
    fn srgb_primaries_reproduce_the_lab_matrix() {
        let m = rgb_to_xyz_from_primaries(0.64, 0.33, 0.30, 0.60, 0.15, 0.06, D65_XYZ).unwrap();
        assert_matrix_close(m, SRGB_TO_XYZ, 1e-6);
        // White maps exactly to the white point.
        assert_vec_close(mul_vec(m, [1.0, 1.0, 1.0]), D65_XYZ, 1e-6);
    }

    #[test]
    fn degenerate_primaries_are_singular() {
        // A primary on the x axis has y = 0 and no XYZ representation.
        assert_eq!(
            rgb_to_xyz_from_primaries(0.64, 0.0, 0.30, 0.60, 0.15, 0.06, D65_XYZ),
            Err(CmsError::Singular)
        );
        // Three primaries on one line enclose no gamut.
        assert_eq!(
            rgb_to_xyz_from_primaries(0.1, 0.1, 0.2, 0.2, 0.3, 0.3, D65_XYZ),
            Err(CmsError::Singular)
        );
    }

    #[test]
    fn rec2020_to_xyz_matches_bt2020() {
        // ITU-R BT.2020 derived matrix, as published with the (0.3127,
        // 0.3290) D65. Our D65 differs from that at the fourth decimal (see
        // `D65_XYZ`), which moves the blue column of the Z row by about
        // 2e-4; the contract fixes the first row to 1e-4.
        let want = [
            [0.636_958, 0.144_617, 0.168_881],
            [0.262_700, 0.677_998, 0.059_302],
            [0.000_000, 0.028_073, 1.060_985],
        ];
        let got = *REC2020_TO_XYZ;
        assert_vec_close(got[0], [0.6370, 0.1446, 0.1689], 1e-4);
        assert_matrix_close(got, want, 3e-4);
        // Z of red is exactly zero for these primaries only up to rounding.
        assert!(got[2][0].abs() < 1e-6);
        assert_vec_close(mul_vec(got, [1.0, 1.0, 1.0]), D65_XYZ, 1e-6);
    }

    #[test]
    fn xyz_to_rec2020_is_the_inverse() {
        assert_matrix_close(mul(*XYZ_TO_REC2020, *REC2020_TO_XYZ), IDENTITY, 1e-6);
        assert_matrix_close(*XYZ_TO_REC2020, inverse(*REC2020_TO_XYZ).unwrap(), 1e-6);
        // Published XYZ to Rec.2020 (BT.2020 / BT.2087), same tolerance note.
        let want = [
            [1.716_651, -0.355_671, -0.253_366],
            [-0.666_684, 1.616_481, 0.015_769],
            [0.017_640, -0.042_771, 0.942_103],
        ];
        assert_matrix_close(*XYZ_TO_REC2020, want, 5e-4);
    }

    #[test]
    fn bradford_d50_to_d65_matches_the_standard() {
        let got = bradford_adaptation(D50_XYZ, D65_XYZ);
        let want = [
            [0.955_576_6, -0.023_039_3, 0.063_163_6],
            [-0.028_289_5, 1.009_941_6, 0.021_007_7],
            [0.012_298_2, -0.020_483_0, 1.329_909_8],
        ];
        assert_vec_close(got[0], [0.955_576_6, -0.023_039_3, 0.063_163_6], 1e-4);
        assert_matrix_close(got, want, 1e-4);
        // It takes the D50 white to the D65 white, and composing with the
        // reverse adaptation gives the identity.
        assert_vec_close(mul_vec(got, D50_XYZ), D65_XYZ, 1e-5);
        let back = bradford_adaptation(D65_XYZ, D50_XYZ);
        assert_matrix_close(mul(back, got), IDENTITY, 1e-6);
    }

    #[test]
    fn bradford_between_equal_whites_is_identity() {
        assert_matrix_close(bradford_adaptation(D65_XYZ, D65_XYZ), IDENTITY, 1e-7);
    }

    #[test]
    fn camera_to_rec2020_of_identity_is_xyz_to_rec2020_times_the_white() {
        // With the identity matrix the camera's neutral is the D65 XYZ, so
        // the white normalisation is diag(D65).
        let got = camera_to_rec2020(IDENTITY, D65_XYZ).unwrap();
        let d65 = [
            [D65_XYZ[0], 0.0, 0.0],
            [0.0, D65_XYZ[1], 0.0],
            [0.0, 0.0, D65_XYZ[2]],
        ];
        assert_matrix_close(got, mul(*XYZ_TO_REC2020, d65), 1e-6);
        assert_vec_close(mul_vec(got, [1.0, 1.0, 1.0]), [1.0, 1.0, 1.0], 1e-5);
    }

    #[test]
    fn camera_to_rec2020_maps_srgb_into_rec2020() {
        // A camera whose matrix happens to be XYZ-to-sRGB: its D65 neutral
        // is (1, 1, 1), so the white normalisation is the identity and the
        // entry matrix is the well known sRGB to Rec.2020 matrix (ITU-R
        // BT.2087).
        let xyz_to_srgb = inverse(SRGB_TO_XYZ).unwrap();
        let got = camera_to_rec2020(xyz_to_srgb, D65_XYZ).unwrap();
        let want = [
            [0.6274, 0.3293, 0.0433],
            [0.0691, 0.9195, 0.0114],
            [0.0164, 0.0880, 0.8956],
        ];
        assert_matrix_close(got, want, 1e-3);
        assert_vec_close(mul_vec(got, [1.0, 1.0, 1.0]), [1.0, 1.0, 1.0], 1e-5);
    }

    #[test]
    fn camera_to_rec2020_keeps_an_unbalanced_camera_neutral() {
        // A camera whose D65 neutral is (0.5, 1, 0.7): XYZ-to-sRGB with its
        // rows scaled. Camera grey (0.18, 0.18, 0.18), which is what a
        // neutral looks like after inversion and step 6, must land on
        // Rec.2020 grey; without the white normalisation it comes out
        // strongly coloured.
        let xyz_to_srgb = inverse(SRGB_TO_XYZ).unwrap();
        let rows = [0.5, 1.0, 0.7];
        let x2c = [0, 1, 2].map(|i| xyz_to_srgb[i].map(|v| v * rows[i]));
        assert_vec_close(mul_vec(x2c, D65_XYZ), rows, 1e-5);
        let got = camera_to_rec2020(x2c, D65_XYZ).unwrap();
        assert_vec_close(mul_vec(got, [0.18; 3]), [0.18; 3], 1e-4);
        // The unnormalised product does not.
        let plain = mul(
            *XYZ_TO_REC2020,
            mul(bradford_adaptation(D65_XYZ, D65_XYZ), inverse(x2c).unwrap()),
        );
        let p = mul_vec(plain, [0.18; 3]);
        let spread =
            p.iter().fold(f32::MIN, |m, &v| m.max(v)) - p.iter().fold(f32::MAX, |m, &v| m.min(v));
        assert!(spread > 0.05, "{p:?}");
    }

    #[test]
    fn camera_to_rec2020_adapts_a_non_d65_matrix() {
        // The identity "camera" measured under D50: camera (1, 1, 1) is the
        // D50 white, which Bradford takes to D65 and so to Rec.2020 white.
        let got = camera_to_rec2020(IDENTITY, D50_XYZ).unwrap();
        assert_vec_close(mul_vec(got, [1.0, 1.0, 1.0]), [1.0, 1.0, 1.0], 1e-5);
        // A camera with a D50 matrix and an unbalanced neutral, too.
        let x2c = [[0.8, 0.1, 0.0], [-0.1, 1.2, 0.1], [0.0, 0.1, 0.5]];
        let got = camera_to_rec2020(x2c, D50_XYZ).unwrap();
        assert_vec_close(mul_vec(got, [0.4; 3]), [0.4; 3], 1e-5);
    }

    #[test]
    fn camera_to_rec2020_rejects_a_singular_matrix() {
        assert_eq!(
            camera_to_rec2020([[0.0; 3]; 3], D65_XYZ),
            Err(CmsError::Singular)
        );
        // Invertible, but blind to the white in one channel.
        let blind = [
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [-1.0, 0.0, 0.95047 / 1.08883],
        ];
        assert_eq!(camera_to_rec2020(blind, D65_XYZ), Err(CmsError::Singular));
    }
}
