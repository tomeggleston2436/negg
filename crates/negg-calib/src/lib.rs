// SPDX-License-Identifier: MPL-2.0

//! Rig profiles: flat-field correction, channel crosstalk and the
//! camera-to-density fit.
//!
//! Owns calibration maths and profile data. Must not decode files,
//! do I/O or render images. Step 2 of `docs/MATHS.md` defines the flat
//! field and the crosstalk unmix; the density fit is the "Camera-to-density
//! fit" part of the same step (R-density-fit).

#![allow(clippy::needless_range_loop)]

use std::fmt;

use nalgebra::{DMatrix, DVector, Matrix3};
use negg_film::analysis_grid;
use negg_types::Image;
use negg_types::profile::{DensityModelKind, RigProfile};
use negg_types::rig::{
    DensityModel, FLAT_GAIN_MAX, FLAT_GAIN_MIN, FLAT_SIGMA_DIVISOR, FlatField, RigCorrection,
};

/// Long side, in cells, of the grid the flat is reduced to (R-flat).
const FLAT_GRID_LONG_SIDE: usize = 1024;
/// A grid cell with any channel at or above this level counts as clipped
/// (R-flat). Controller choice, TODO(measure).
const FLAT_CLIP_LEVEL: f32 = 0.98;
/// The flat is rejected when more than this fraction of cells is clipped
/// (R-flat). Controller choice, TODO(measure).
const FLAT_CLIP_FRACTION: f64 = 0.01;
/// `|det M|` below this is a singular crosstalk matrix (R-crosstalk).
/// Controller choice, TODO(measure).
const CROSSTALK_MIN_DET: f64 = 1e-6;
/// Transmittance floor before the log (R-density-fit), as in step 3.
const LINEAR_FLOOR: f32 = 1e-6;
/// Fewest patches for the full density model (R-density-fit).
const FULL_MODEL_MIN_PATCHES: usize = 8;
/// Largest design matrix condition number for the full model
/// (R-density-fit). Controller choice, TODO(measure).
const FULL_MODEL_MAX_CONDITION: f64 = 100.0;
/// Fewest patches for the diagonal model (R-density-fit).
const DIAGONAL_MODEL_MIN_PATCHES: usize = 3;

/// Why a calibration could not be computed.
#[derive(Clone, Debug, PartialEq)]
pub enum CalibError {
    /// Too few patches for the fit.
    TooFewPatches {
        /// Patches needed.
        need: usize,
        /// Patches supplied.
        got: usize,
    },
    /// A matrix or normal system is singular.
    Singular,
    /// The flat frame is clipped (R-flat).
    Clipped(String),
    /// A frame does not have the size the profile records.
    SizeMismatch {
        /// Width and height the profile requires.
        expected: (usize, usize),
        /// Width and height supplied.
        got: (usize, usize),
    },
    /// Any other invalid input, with the reason.
    Invalid(String),
}

impl fmt::Display for CalibError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CalibError::TooFewPatches { need, got } => {
                write!(f, "too few patches: need {need}, got {got}")
            }
            CalibError::Singular => write!(f, "singular matrix"),
            CalibError::Clipped(msg) => write!(f, "clipped flat field: {msg}"),
            CalibError::SizeMismatch { expected, got } => write!(
                f,
                "frame size mismatch: expected {}x{}, got {}x{}",
                expected.0, expected.1, got.0, got.1
            ),
            CalibError::Invalid(msg) => write!(f, "invalid calibration input: {msg}"),
        }
    }
}

impl std::error::Error for CalibError {}

/// Maps index `i` (possibly outside `0..n`) into `0..n` by symmetric
/// reflection: -1 maps to 0, -2 to 1, `n` to `n - 1`, repeated for any
/// distance.
fn reflect(i: isize, n: usize) -> usize {
    let n = n as isize;
    let period = 2 * n;
    let m = i.rem_euclid(period);
    (if m < n { m } else { period - 1 - m }) as usize
}

/// Normalised one-dimensional Gaussian kernel of radius `ceil(3 sigma)`.
fn gaussian_kernel(sigma: f32) -> Vec<f32> {
    let radius = (3.0 * sigma).ceil().max(0.0) as isize;
    let mut k: Vec<f64> = (-radius..=radius)
        .map(|d| (-(f64::from(d as i32).powi(2)) / (2.0 * f64::from(sigma).powi(2))).exp())
        .collect();
    let sum: f64 = k.iter().sum();
    for v in &mut k {
        *v /= sum;
    }
    k.into_iter().map(|v| v as f32).collect()
}

/// Separable Gaussian blur of a grid image with symmetric edge reflection.
///
/// Step 2 of `docs/MATHS.md` (R-flat). The kernel radius is
/// `ceil(3 * sigma_cells)` and the weights sum to 1. A `sigma_cells` that is
/// not finite or not positive returns the grid unchanged.
pub fn gaussian_blur_grid(grid: &Image, sigma_cells: f32) -> Image {
    let (w, h) = (grid.width(), grid.height());
    if w == 0 || h == 0 || !sigma_cells.is_finite() || sigma_cells <= 0.0 {
        return grid.clone();
    }
    let kernel = gaussian_kernel(sigma_cells);
    let radius = (kernel.len() / 2) as isize;
    let src = grid.data();

    let mut tmp = Image::new(w, h);
    {
        let dst = tmp.data_mut();
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0.0f64; 3];
                for (ki, &kw) in kernel.iter().enumerate() {
                    let sx = reflect(x as isize + ki as isize - radius, w);
                    let base = (y * w + sx) * 3;
                    for c in 0..3 {
                        acc[c] += f64::from(kw) * f64::from(src[base + c]);
                    }
                }
                let o = (y * w + x) * 3;
                for c in 0..3 {
                    dst[o + c] = acc[c] as f32;
                }
            }
        }
    }
    let mut out = Image::new(w, h);
    {
        let src = tmp.data();
        let dst = out.data_mut();
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0.0f64; 3];
                for (ki, &kw) in kernel.iter().enumerate() {
                    let sy = reflect(y as isize + ki as isize - radius, h);
                    let base = (sy * w + x) * 3;
                    for c in 0..3 {
                        acc[c] += f64::from(kw) * f64::from(src[base + c]);
                    }
                }
                let o = (y * w + x) * 3;
                for c in 0..3 {
                    dst[o + c] = acc[c] as f32;
                }
            }
        }
    }
    out
}

/// Builds the flat-field gains from a blank-light frame and returns them
/// with the per-channel mean of the unblurred grid block medians.
///
/// Step 2 of `docs/MATHS.md` (R-flat). The flat is reduced to the 1024 px
/// analysis grid, blurred with `sigma_cells = (long_side_px / 64) / block`,
/// and `gain = clamp(mean / blur, 0.25, 4)` (a zero blur gives 4).
///
/// # Errors
///
/// [`CalibError::Invalid`] for an empty or non-finite flat, and
/// [`CalibError::Clipped`] when more than 1% of grid cells have any channel
/// at or above 0.98.
pub fn flat_field(flat: &Image) -> Result<(FlatField, [f32; 3]), CalibError> {
    let (w, h) = (flat.width(), flat.height());
    if w == 0 || h == 0 {
        return Err(CalibError::Invalid("flat frame is empty".into()));
    }
    if flat.data().iter().any(|v| !v.is_finite()) {
        return Err(CalibError::Invalid(
            "flat frame has non-finite samples".into(),
        ));
    }
    let grid = analysis_grid(flat, FLAT_GRID_LONG_SIDE);
    let med = &grid.median;
    let cells = grid.cells();

    let clipped = med
        .data()
        .as_chunks::<3>()
        .0
        .iter()
        .filter(|p| p.iter().any(|&v| v >= FLAT_CLIP_LEVEL))
        .count();
    if clipped as f64 > FLAT_CLIP_FRACTION * cells as f64 {
        return Err(CalibError::Clipped(format!(
            "{clipped} of {cells} grid cells have a channel at or above {FLAT_CLIP_LEVEL}"
        )));
    }

    let mut mean = [0.0f64; 3];
    for p in med.data().as_chunks::<3>().0.iter() {
        for c in 0..3 {
            mean[c] += f64::from(p[c]);
        }
    }
    let mean = mean.map(|s| (s / cells as f64) as f32);

    let long_side = w.max(h) as f32;
    let sigma_cells = (long_side / FLAT_SIGMA_DIVISOR) / grid.block as f32;
    let blur = gaussian_blur_grid(med, sigma_cells);

    let mut gain = Image::new(med.width(), med.height());
    for (g, b) in gain
        .data_mut()
        .as_chunks_mut::<3>()
        .0
        .iter_mut()
        .zip(blur.data().as_chunks::<3>().0.iter())
    {
        for c in 0..3 {
            g[c] = if b[c] > 0.0 {
                (mean[c] / b[c]).clamp(FLAT_GAIN_MIN, FLAT_GAIN_MAX)
            } else {
                FLAT_GAIN_MAX
            };
        }
    }
    Ok((
        FlatField {
            gain,
            width: w,
            height: h,
        },
        mean,
    ))
}

fn to_matrix3(m: [[f32; 3]; 3]) -> Matrix3<f64> {
    Matrix3::from_fn(|r, c| f64::from(m[r][c]))
}

fn from_matrix3(m: &Matrix3<f64>) -> [[f32; 3]; 3] {
    let mut out = [[0.0f32; 3]; 3];
    for (r, row) in out.iter_mut().enumerate() {
        for (c, v) in row.iter_mut().enumerate() {
            *v = m[(r, c)] as f32;
        }
    }
    out
}

/// Builds the crosstalk matrix `M` from the three band responses.
///
/// Step 2 of `docs/MATHS.md` (R-crosstalk). `responses[j]` is the
/// per-channel response `r_j` to band `j` lit alone; column `j` of the
/// result is `r_j / r_j[j]`, so `M[c][j] = responses[j][c] / responses[j][j]`.
///
/// # Errors
///
/// [`CalibError::Invalid`] when a response is non-finite, `r_j[j]` is not
/// positive or not the largest channel of `r_j` (frames swapped or not
/// narrow band), and [`CalibError::Singular`] when `|det M| < 1e-6`.
pub fn crosstalk_matrix(responses: [[f32; 3]; 3]) -> Result<[[f32; 3]; 3], CalibError> {
    let mut m = [[0.0f32; 3]; 3];
    for (j, r) in responses.iter().enumerate() {
        if r.iter().any(|v| !v.is_finite()) {
            return Err(CalibError::Invalid(format!(
                "band {j} response is not finite"
            )));
        }
        let own = r[j];
        if own <= 0.0 || r.iter().any(|&v| v > own) {
            return Err(CalibError::Invalid(format!(
                "band {j} response {r:?} does not peak in channel {j}: frames swapped or not narrow band"
            )));
        }
        for c in 0..3 {
            m[c][j] = r[c] / own;
        }
    }
    if to_matrix3(m).determinant().abs() < CROSSTALK_MIN_DET {
        return Err(CalibError::Singular);
    }
    Ok(m)
}

/// Inverts the crosstalk matrix, `M^-1` (applied as `I <- max(M^-1 I, 0)`).
///
/// Step 2 of `docs/MATHS.md` (R-crosstalk).
///
/// # Errors
///
/// [`CalibError::Singular`] when `|det M| < 1e-6` or the matrix is not
/// finite.
pub fn unmix_matrix(m: [[f32; 3]; 3]) -> Result<[[f32; 3]; 3], CalibError> {
    if m.iter().flatten().any(|v| !v.is_finite()) {
        return Err(CalibError::Singular);
    }
    let mat = to_matrix3(m);
    if mat.determinant().abs() < CROSSTALK_MIN_DET {
        return Err(CalibError::Singular);
    }
    let inv = mat.try_inverse().ok_or(CalibError::Singular)?;
    Ok(from_matrix3(&inv))
}

/// One wedge patch: the rig-corrected linear camera value and the certified
/// density (R-density-fit).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DensitySample {
    /// `patch_median` of the rig-corrected wedge grid.
    pub linear: [f32; 3],
    /// Certified transmission density per channel.
    pub density: [f32; 3],
}

/// The result of [`fit_density`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DensityFit {
    /// The fitted model.
    pub model: DensityModel,
    /// Whether the cross terms were fitted.
    pub kind: DensityModelKind,
    /// Root mean square of the 3N residuals, in log-D.
    pub residual_rms: f32,
    /// Number of patches used.
    pub n_patches: u32,
    /// Condition number of the centred `N x 3` design matrix of `log10 I`
    /// (infinite when rank deficient). The caller warns when `kind` is
    /// [`DensityModelKind::Diagonal`].
    pub condition: f32,
}

fn log_floor(v: f32) -> f64 {
    f64::from(v.max(LINEAR_FLOOR)).log10()
}

/// Fits the camera-to-density model `D_c = a_c + sum_k G_ck log10(max(I_k, 1e-6))`.
///
/// Step 2 of `docs/MATHS.md` (R-density-fit, camera-to-density fit). Full
/// model (least squares by SVD on the centred design) when `N >= 8` and the
/// design condition number is at most 100; otherwise the diagonal model
/// (`G` diagonal, fitting `a_c` and `b_c`) when `N >= 3`.
///
/// # Errors
///
/// [`CalibError::TooFewPatches`] when `N < 3`, [`CalibError::Invalid`] for
/// non-finite samples or any fitted `b_c >= 0`, and [`CalibError::Singular`]
/// when a diagonal fit has no spread in its own channel.
pub fn fit_density(samples: &[DensitySample]) -> Result<DensityFit, CalibError> {
    let n = samples.len();
    if n < DIAGONAL_MODEL_MIN_PATCHES {
        return Err(CalibError::TooFewPatches {
            need: DIAGONAL_MODEL_MIN_PATCHES,
            got: n,
        });
    }
    if samples
        .iter()
        .any(|s| s.linear.iter().chain(&s.density).any(|v| !v.is_finite()))
    {
        return Err(CalibError::Invalid("non-finite density sample".into()));
    }

    let x = DMatrix::from_fn(n, 3, |i, k| log_floor(samples[i].linear[k]));
    let col_mean: Vec<f64> = (0..3).map(|k| x.column(k).mean()).collect();
    let centred = DMatrix::from_fn(n, 3, |i, k| x[(i, k)] - col_mean[k]);
    let sv = centred.clone().svd(false, false).singular_values;
    let (smax, smin) = (sv.max(), sv.min());
    let condition = if smin > 0.0 && smax.is_finite() {
        smax / smin
    } else {
        f64::INFINITY
    };

    let d_mean: Vec<f64> = (0..3)
        .map(|c| samples.iter().map(|s| f64::from(s.density[c])).sum::<f64>() / n as f64)
        .collect();

    let full = n >= FULL_MODEL_MIN_PATCHES && condition <= FULL_MODEL_MAX_CONDITION;
    let mut g = [[0.0f64; 3]; 3];
    let mut a = [0.0f64; 3];

    if full {
        let svd = centred.svd(true, true);
        for c in 0..3 {
            let y = DVector::from_fn(n, |i, _| f64::from(samples[i].density[c]) - d_mean[c]);
            let sol = svd.solve(&y, 1e-12).map_err(|_| CalibError::Singular)?;
            for k in 0..3 {
                g[c][k] = sol[k];
            }
            a[c] = d_mean[c] - (0..3).map(|k| g[c][k] * col_mean[k]).sum::<f64>();
        }
    } else {
        for c in 0..3 {
            let xc = centred.column(c);
            let sxx = xc.dot(&xc);
            if sxx <= 0.0 {
                return Err(CalibError::Singular);
            }
            let sxy: f64 = (0..n)
                .map(|i| xc[i] * (f64::from(samples[i].density[c]) - d_mean[c]))
                .sum();
            g[c][c] = sxy / sxx;
            a[c] = d_mean[c] - g[c][c] * col_mean[c];
        }
    }

    for (c, row) in g.iter().enumerate() {
        if row[c] >= 0.0 {
            return Err(CalibError::Invalid(format!(
                "fitted slope b_{c} = {} is not negative",
                row[c]
            )));
        }
    }

    let mut sq = 0.0f64;
    for i in 0..n {
        for c in 0..3 {
            let pred = a[c] + (0..3).map(|k| g[c][k] * x[(i, k)]).sum::<f64>();
            let r = pred - f64::from(samples[i].density[c]);
            sq += r * r;
        }
    }
    let residual_rms = (sq / (3 * n) as f64).sqrt() as f32;

    Ok(DensityFit {
        model: DensityModel {
            a: a.map(|v| v as f32),
            g: g.map(|row| row.map(|v| v as f32)),
        },
        kind: if full {
            DensityModelKind::Full
        } else {
            DensityModelKind::Diagonal
        },
        residual_rms,
        n_patches: n as u32,
        condition: condition as f32,
    })
}

/// Builds the runtime [`RigCorrection`] from a rig profile and, when the
/// profile has a `[flat_field]` table, its decoded flat frame.
///
/// Step 2 of `docs/MATHS.md` (R-flat, R-crosstalk, R-density-use). The flat
/// gain is recomputed from the frame (the table's `mean` is informational).
/// The unmix is `M^-1` of the table's `m`. A flat frame passed to a profile
/// without a flat field table is ignored.
///
/// # Errors
///
/// [`CalibError::Invalid`] when the profile fails validation or the flat
/// frame is missing, [`CalibError::SizeMismatch`] when the frame size differs
/// from the table, and any error of [`flat_field`] or [`unmix_matrix`].
pub fn rig_correction(
    profile: &RigProfile,
    flat_frame: Option<&Image>,
) -> Result<RigCorrection, CalibError> {
    profile
        .validate()
        .map_err(|e| CalibError::Invalid(e.to_string()))?;

    let flat = match &profile.flat_field {
        None => None,
        Some(table) => {
            let frame = flat_frame.ok_or_else(|| {
                CalibError::Invalid(format!(
                    "profile {:?} has a flat field table but no flat frame was supplied",
                    profile.name
                ))
            })?;
            let expected = (table.width as usize, table.height as usize);
            let got = (frame.width(), frame.height());
            if expected != got {
                return Err(CalibError::SizeMismatch { expected, got });
            }
            Some(flat_field(frame)?.0)
        }
    };
    let unmix = profile
        .crosstalk
        .as_ref()
        .map(|t| unmix_matrix(t.m))
        .transpose()?;

    Ok(RigCorrection {
        name: profile.name.clone(),
        flat,
        unmix,
        density: profile.density_model(),
    })
}

#[cfg(test)]
mod tests;
