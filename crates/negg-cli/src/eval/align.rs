// SPDX-License-Identifier: Apache-2.0

//! Registration of a reference render onto the engine's render
//! (docs/ADR/0028).
//!
//! A reference made by hand in another tool (Negative Lab Pro above all)
//! is a crop of roughly the same frame at its own size, possibly mirrored
//! (a film scanned base side to the lens) or turned by quarter turns. The
//! eval finds the similarity that maps it onto the render before scoring:
//! one of the 8 orientations (4 quarter turns, each with or without a
//! mirror), a uniform scale, a translation and a small rotation of at most
//! [`MAX_ROTATION_DEGREES`] (hand straightening in the editor turns every
//! real export by a degree or so). Uniform, because neither tool resamples
//! one axis on its own.
//!
//! The model: a render pixel centre `(x, y)` (full-resolution pixel
//! indices) sees the oriented reference at `u = (x - offset[0]) / scale`,
//! `v = (y - offset[1]) / scale`, where the oriented reference is the
//! reference rotated clockwise by `rotate_degrees` and then mirrored left
//! to right when `flip_horizontal`, exactly as [`negg_types::Geometry`]
//! orders them, and then turned clockwise by `angle_degrees` about its own
//! centre (so `offset` is where the top-left pixel centre would land
//! without that turn). So `scale` is render pixels per reference pixel and
//! `offset` is where the centre of the oriented reference's top-left pixel
//! lands on the render.
//!
//! The search maximises a normalised (zero-mean) cross-correlation over
//! the overlap, coarse to fine, on luminance pyramids built by box
//! averaging (the render's linear RGB, the reference's encoded RGB) and
//! converting each block to L*. The objective is the correlation of L*,
//! and, on levels with a long side of at least [`EDGE_MIN_LONG_SIDE`], the
//! mean of that and the correlation of the gradient magnitude of L*:
//!
//! 1. At the coarsest level (long side at most [`COARSE_LONG_SIDE`]), an
//!    exhaustive search per orientation on the L* correlation alone:
//!    scales whose reference width covers [`MIN_COVERAGE`] to
//!    [`MAX_COVERAGE`] of the render width in geometric steps of one
//!    coarse pixel at the far edge, and every whole coarse-pixel
//!    translation that keeps the reference within [`MARGIN`] of the
//!    render. A candidate counts only when its overlap covers at least
//!    [`MIN_OVERLAP`] of the render and at least [`MIN_REFERENCE_INSIDE`]
//!    of the reference, and the reference is at most [`MAX_COVERAGE`] of
//!    the render in either direction (so a landscape reference of a
//!    landscape render is never placed upright). The best
//!    [`COARSE_SEEDS`] distinct placements of each orientation survive,
//!    not only the best: on a dark or smooth scene the coarse surface is a
//!    flat ridge whose top can be several coarse pixels from the truth.
//! 2. The seeds are not climbed on the coarsest level (the climb would
//!    slide along that ridge). They start the climb on the next level, each
//!    also at the turns of [`ANGLE_SEEDS`], and are refined on every level
//!    down to the finest (by factors of two, up to a long side of
//!    [`FINE_LONG_SIDE`]) by a coordinate hill climb on scale (about the
//!    reference centre), both offsets and the angle, from one level pixel
//!    down to a quarter (an eighth on the finest level). The field is
//!    narrowed to the best [`COARSE_SURVIVORS`] after the first climbed
//!    level and the best [`SURVIVORS`] after each finer one, merging climbs
//!    that ended on the same placement.
//!
//! The verdict is a second correlation at the best placement on the
//! finest level, of the gradient magnitude of L* rather than L* itself:
//! the smooth shading any two pictures share can give two unrelated
//! images an L* correlation above 0.6, while their edges do not line up
//! (the synthetic tests give 0.07 to 0.22 for unrelated scenes against 0.9
//! and more for the same scene). A reference whose edge correlation is
//! below [`MIN_CORRELATION`] is not aligned. Both correlations are
//! invariant to the gain and offset of either image, so a reference with
//! another tone curve or white balance still registers; only the overlap
//! is compared, so the render's crop and the reference's crop may differ
//! freely within the coverage range.
//!
//! Every sum is accumulated in a fixed order (row bands collected in
//! order, then added in sequence), so the result does not depend on the
//! thread count and the eval stays byte-reproducible.

use negg_cms::lab::{rec2020_linear_to_lab, srgb_to_lab};
use negg_types::Image;
use negg_types::image::CHANNELS;
use rayon::prelude::*;

/// Smallest fraction of the render width the oriented reference may
/// cover. A hand crop in another tool rarely removes more than a fifth on
/// each side; 0.6 leaves room for a tight crop of a frame we render with
/// its border. Controller choice, TODO(measure).
pub const MIN_COVERAGE: f64 = 0.6;

/// Largest fraction of the render width the oriented reference may cover:
/// a little over 1, because the reference may keep a sliver of border the
/// render's own crop removed. Controller choice, TODO(measure).
pub const MAX_COVERAGE: f64 = 1.1;

/// How far, as a fraction of the render's width or height, the reference
/// may reach past the render's edge during the search. Controller choice,
/// TODO(measure).
pub const MARGIN: f64 = 0.05;

/// Smallest fraction of the render a candidate must cover to be
/// considered at all, so that a tiny overlap cannot win on a chance
/// correlation. Controller choice, TODO(measure).
pub const MIN_OVERLAP: f64 = 0.3;

/// Smallest fraction of the oriented reference that must lie on the
/// render. The reference is a crop of roughly the same frame, so nearly
/// all of it is on the render; without this bound a small corner of the
/// reference, blown up, can fit a smooth render by chance. Controller
/// choice, TODO(measure).
pub const MIN_REFERENCE_INSIDE: f64 = 0.75;

/// The smallest edge correlation ([`Registration::correlation`]) at which
/// a reference counts as aligned. The synthetic tests give 0.9 to 0.99 for
/// the same scene (lower when the reference is exported smaller and loses
/// fine edges) and 0.07 to 0.22 for unrelated scenes. Controller choice,
/// TODO(measure) on the first real Negative Lab Pro references.
pub const MIN_CORRELATION: f32 = 0.5;

/// Largest rotation, in degrees either way, the search may add to the
/// orientation. Hand straightening in an editor turns a frame by a degree
/// or so; 2 leaves room for a loose horizon. Controller choice (Ruling
/// 108), TODO(measure).
pub const MAX_ROTATION_DEGREES: f64 = 2.0;

/// Smallest long side, in pixels, of a pyramid level on which the climb
/// also turns the reference: the coarsest level's, so every level the
/// climb visits turns it unless the whole image is tiny. Controller choice
/// (Ruling 108), TODO(measure).
const ROTATION_MIN_LONG_SIDE: usize = COARSE_LONG_SIDE;

/// Starting angles, in degrees, of the climb on the first level below the
/// coarsest. Controller choice (Ruling 108), TODO(measure).
const ANGLE_SEEDS: [f64; 3] = [-1.2, 0.0, 1.2];

/// Two climbs that end within this many degrees of each other, on the
/// same placement, count as one. Controller choice, TODO(measure).
const ANGLE_SEPARATION_DEGREES: f64 = 0.3;

/// Smallest long side, in pixels, of a pyramid level on which the search
/// also correlates the gradient magnitude. On coarser levels the edges of
/// a textured scene are blurred into noise that pulls the climb off the
/// L* peak (the 360 pixel synthetic frames: a 5 pixel error at 90 pixels
/// wide). Controller choice (Ruling 108), TODO(measure).
const EDGE_MIN_LONG_SIDE: usize = 3 * COARSE_LONG_SIDE;

/// Long side of the coarsest pyramid level, where the exhaustive search
/// runs.
pub const COARSE_LONG_SIDE: usize = 48;

/// Largest long side of the finest pyramid level the search refines on.
/// Sub-pixel steps there give about one full-resolution pixel at 6000
/// pixels wide.
pub const FINE_LONG_SIDE: usize = 1536;

/// Placements kept per orientation from the exhaustive coarse search. On a
/// dark or smooth scene the coarse correlation surface is a flat ridge
/// whose maximum can lie a few coarse pixels from the true placement (the
/// synthetic dark-ramp test), so the best placement alone is not enough:
/// the best few distinct ones go on to the finer levels, where the edges
/// decide. Controller choice (Ruling 108), TODO(measure).
const COARSE_SEEDS: usize = 3;

/// Two seeds of one orientation are distinct when their offsets differ by
/// more than this many coarse pixels on an axis. Controller choice,
/// TODO(measure).
const SEED_SEPARATION: f64 = 1.5;

/// ... or their scales differ by more than this fraction. Controller
/// choice, TODO(measure).
const SEED_SCALE_SEPARATION: f64 = 0.04;

/// Candidates kept after the hill climb on the level below the coarsest
/// (the seeds of all 8 orientations enter it).
const COARSE_SURVIVORS: usize = 6;

/// Candidates kept after the hill climb on each finer level.
const SURVIVORS: usize = 2;

/// Below this many level pixels a correlation is summed on the calling
/// thread; the bands and their order are the same either way.
const PARALLEL_PIXELS: f64 = 65536.0;

/// Rows per band in the parallel sums.
const BAND_ROWS: usize = 16;

/// One of the 8 orientations: rotate clockwise by `rotate_degrees`, then
/// mirror left to right when `flip_horizontal`, as
/// [`negg_types::Geometry`] does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Orientation {
    /// 0, 90, 180 or 270.
    pub rotate_degrees: u16,
    /// Mirror after the rotation.
    pub flip_horizontal: bool,
}

/// The 8 orientations in search order (ties go to the earlier one).
pub const ORIENTATIONS: [Orientation; 8] = [
    Orientation {
        rotate_degrees: 0,
        flip_horizontal: false,
    },
    Orientation {
        rotate_degrees: 0,
        flip_horizontal: true,
    },
    Orientation {
        rotate_degrees: 90,
        flip_horizontal: false,
    },
    Orientation {
        rotate_degrees: 90,
        flip_horizontal: true,
    },
    Orientation {
        rotate_degrees: 180,
        flip_horizontal: false,
    },
    Orientation {
        rotate_degrees: 180,
        flip_horizontal: true,
    },
    Orientation {
        rotate_degrees: 270,
        flip_horizontal: false,
    },
    Orientation {
        rotate_degrees: 270,
        flip_horizontal: true,
    },
];

impl Orientation {
    /// Whether the orientation swaps width and height.
    fn swaps(self) -> bool {
        matches!(self.rotate_degrees, 90 | 270)
    }

    /// The oriented size of a `w` by `h` image.
    pub fn oriented_size(self, w: usize, h: usize) -> (usize, usize) {
        if self.swaps() { (h, w) } else { (w, h) }
    }

    /// Maps continuous pixel-index coordinates `(u, v)` of the oriented
    /// image back to the original `w` by `h` image: the inverse of
    /// [`negg_types::Geometry::apply`]'s pixel move, extended to
    /// fractional coordinates. As an affine map.
    fn to_original(self, w: usize, h: usize) -> Affine {
        let (w1, h1) = (w as f64 - 1.0, h as f64 - 1.0);
        let (ow, _) = self.oriented_size(w, h);
        // Undo the flip: u' = ow - 1 - u.
        let unflip = if self.flip_horizontal {
            Affine([[-1.0, 0.0, ow as f64 - 1.0], [0.0, 1.0, 0.0]])
        } else {
            Affine::IDENTITY
        };
        let unrotate = match self.rotate_degrees {
            90 => Affine([[0.0, 1.0, 0.0], [-1.0, 0.0, h1]]),
            180 => Affine([[-1.0, 0.0, w1], [0.0, -1.0, h1]]),
            270 => Affine([[0.0, -1.0, w1], [1.0, 0.0, 0.0]]),
            _ => Affine::IDENTITY,
        };
        unrotate.after(&unflip)
    }
}

/// An affine map of the plane, `[[a, b, c], [d, e, f]]` taking `(x, y)`
/// to `(a x + b y + c, d x + e y + f)`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Affine([[f64; 3]; 2]);

impl Affine {
    const IDENTITY: Affine = Affine([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);

    /// The map `(x, y) -> (k x + c, k y + c)`.
    fn scale_shift(k: f64, cx: f64, cy: f64) -> Affine {
        Affine([[k, 0.0, cx], [0.0, k, cy]])
    }

    fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        let m = &self.0;
        (
            m[0][0] * x + m[0][1] * y + m[0][2],
            m[1][0] * x + m[1][1] * y + m[1][2],
        )
    }

    /// `self` applied after `first`.
    fn after(&self, first: &Affine) -> Affine {
        let (a, b) = (&self.0, &first.0);
        let mut out = [[0.0; 3]; 2];
        for r in 0..2 {
            out[r][0] = a[r][0] * b[0][0] + a[r][1] * b[1][0];
            out[r][1] = a[r][0] * b[0][1] + a[r][1] * b[1][1];
            out[r][2] = a[r][0] * b[0][2] + a[r][1] * b[1][2] + a[r][2];
        }
        Affine(out)
    }
}

/// Where a reference lies on the render: the result of [`register`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Registration {
    /// The orientation that brings the reference to the render's.
    pub orientation: Orientation,
    /// Render pixels per reference pixel.
    pub scale: f64,
    /// Render position (pixel indices, `[x, y]`) of the centre of the
    /// oriented reference's top-left pixel.
    pub offset: [f64; 2],
    /// Small extra rotation of the oriented reference about its centre, in
    /// degrees, clockwise positive, within
    /// `+-`[`MAX_ROTATION_DEGREES`]. Zero for a reference that is only
    /// cropped, scaled, mirrored or turned by quarter turns.
    pub angle_degrees: f64,
    /// Normalised cross-correlation of the gradient magnitude of L* over
    /// the overlap on the finest search level, in `[-1, 1]`: the verdict
    /// ([`Registration::is_aligned`]).
    pub correlation: f64,
    /// Normalised cross-correlation of L* itself there, recomputed on L*
    /// alone at the final placement. The search maximised the mean of this
    /// and the gradient correlation on the finer levels, not this alone.
    pub luminance_correlation: f64,
    /// The reference's own size, `(width, height)`, before orientation.
    pub reference_size: (usize, usize),
}

impl Registration {
    /// Whether the correlation reaches [`MIN_CORRELATION`].
    pub fn is_aligned(&self) -> bool {
        self.correlation >= f64::from(MIN_CORRELATION)
    }

    /// The map from render pixel indices to the oriented reference's pixel
    /// indices. The rotation is about the centre of the oriented reference,
    /// so `offset` keeps its meaning at any angle: where the top-left pixel
    /// centre would land with no rotation.
    fn render_to_oriented(&self) -> Affine {
        if self.angle_degrees == 0.0 {
            return Affine::scale_shift(
                1.0 / self.scale,
                -self.offset[0] / self.scale,
                -self.offset[1] / self.scale,
            );
        }
        let (w, h) = self.reference_size;
        let (ow, oh) = self.orientation.oriented_size(w, h);
        let (cu, cv) = ((ow as f64 - 1.0) / 2.0, (oh as f64 - 1.0) / 2.0);
        let (ox, oy) = (
            self.offset[0] + self.scale * cu,
            self.offset[1] + self.scale * cv,
        );
        let (sn, c) = self.angle_degrees.to_radians().sin_cos();
        let k = self.scale;
        Affine([
            [c / k, sn / k, cu - (c * ox + sn * oy) / k],
            [-sn / k, c / k, cv - (-sn * ox + c * oy) / k],
        ])
    }

    /// The map from render pixel indices to the reference's own
    /// (unoriented) pixel indices.
    fn render_to_reference(&self) -> Affine {
        let (w, h) = self.reference_size;
        self.orientation
            .to_original(w, h)
            .after(&self.render_to_oriented())
    }

    /// The reference pixel coordinates `(p, q)` seen at render pixel
    /// `(x, y)`.
    pub fn to_reference(&self, x: f64, y: f64) -> (f64, f64) {
        self.render_to_reference().apply(x, y)
    }

    /// The render pixels whose centres see the reference within its pixel
    /// centres, as `(x0, y0, x1, y1)` with `x1` and `y1` exclusive, on a
    /// render of `w` by `h`; `None` when there are none. Without rotation
    /// the overlap is a rectangle and this is exactly it. With rotation it
    /// is the largest rectangle about the reference's centre that lies
    /// inside the rotated reference (the same fraction of both half
    /// extents), cut to the render: a subset of the overlap, so that every
    /// pixel in it samples the reference without clamping.
    pub fn overlap_rect(&self, w: usize, h: usize) -> Option<(usize, usize, usize, usize)> {
        let (rw, rh) = self.reference_size;
        let (ow, oh) = self.orientation.oriented_size(rw, rh);
        let axis = |lo: f64, hi: f64, n: usize| -> Option<(usize, usize)> {
            let lo = lo.ceil().max(0.0);
            let hi = hi.floor().min(n as f64 - 1.0);
            if !(lo.is_finite() && hi.is_finite()) || hi < lo {
                return None;
            }
            Some((lo as usize, hi as usize + 1))
        };
        let (ext_w, ext_h) = (
            self.scale * (ow as f64 - 1.0),
            self.scale * (oh as f64 - 1.0),
        );
        if self.angle_degrees == 0.0 {
            let (x0, x1) = axis(self.offset[0], self.offset[0] + ext_w, w)?;
            let (y0, y1) = axis(self.offset[1], self.offset[1] + ext_h, h)?;
            return Some((x0, y0, x1, y1));
        }
        let (sn, c) = self.angle_degrees.to_radians().sin_cos();
        let (sn, c) = (sn.abs(), c.abs());
        // Corners (+-a, +-b) of the inscribed rectangle must satisfy
        // a c + b sn <= W / 2 and a sn + b c <= H / 2.
        let fit = (ext_w / (ext_w * c + ext_h * sn)).min(ext_h / (ext_h * c + ext_w * sn));
        let fit = fit.min(1.0);
        let (cx, cy) = (self.offset[0] + ext_w / 2.0, self.offset[1] + ext_h / 2.0);
        let (a, b) = (fit * ext_w / 2.0, fit * ext_h / 2.0);
        let (x0, x1) = axis(cx - a, cx + a, w)?;
        let (y0, y1) = axis(cy - b, cy + b, h)?;
        Some((x0, y0, x1, y1))
    }

    /// The fraction of a `w` by `h` render inside [`Self::overlap_rect`].
    pub fn overlap_fraction(&self, w: usize, h: usize) -> f64 {
        match self.overlap_rect(w, h) {
            Some((x0, y0, x1, y1)) if w > 0 && h > 0 => {
                ((x1 - x0) * (y1 - y0)) as f64 / (w * h) as f64
            }
            _ => 0.0,
        }
    }
}

/// A single-channel image (L*), row-major.
#[derive(Clone, Debug)]
struct Plane {
    width: usize,
    height: usize,
    data: Vec<f32>,
}

impl Plane {
    /// Bilinear sample at continuous pixel-index coordinates; the caller
    /// keeps `(p, q)` within `[0, width - 1] x [0, height - 1]`.
    fn sample(&self, p: f64, q: f64) -> f64 {
        let p = p.clamp(0.0, (self.width - 1) as f64);
        let q = q.clamp(0.0, (self.height - 1) as f64);
        let x0 = (p.floor() as usize).min(self.width.saturating_sub(2));
        let y0 = (q.floor() as usize).min(self.height.saturating_sub(2));
        let x1 = (x0 + 1).min(self.width - 1);
        let y1 = (y0 + 1).min(self.height - 1);
        let (fx, fy) = (p - x0 as f64, q - y0 as f64);
        let at = |x: usize, y: usize| f64::from(self.data[y * self.width + x]);
        let top = at(x0, y0) * (1.0 - fx) + at(x1, y0) * fx;
        let bottom = at(x0, y1) * (1.0 - fx) + at(x1, y1) * fx;
        top * (1.0 - fy) + bottom * fy
    }

    /// The gradient magnitude by central differences (one-sided at the
    /// edges).
    fn gradient(&self) -> Plane {
        let (w, h) = (self.width, self.height);
        let mut data = vec![0.0f32; w * h];
        data.par_chunks_mut(w.max(1))
            .enumerate()
            .for_each(|(y, row)| {
                let at = |x: usize, y: usize| self.data[y * w + x];
                let (ya, yb) = (y.saturating_sub(1), (y + 1).min(h - 1));
                for (x, out) in row.iter_mut().enumerate() {
                    let (xa, xb) = (x.saturating_sub(1), (x + 1).min(w - 1));
                    let gx = (at(xb, y) - at(xa, y)) / (xb - xa).max(1) as f32;
                    let gy = (at(x, yb) - at(x, ya)) / (yb - ya).max(1) as f32;
                    *out = gx.hypot(gy);
                }
            });
        Plane {
            width: w,
            height: h,
            data,
        }
    }

    /// The 2x2 box average, dropping an odd last row or column.
    fn half(&self) -> Plane {
        let (w, h) = (self.width / 2, self.height / 2);
        let mut data = vec![0.0f32; w * h];
        data.par_chunks_mut(w.max(1))
            .enumerate()
            .for_each(|(y, row)| {
                for (x, out) in row.iter_mut().enumerate() {
                    let at =
                        |dx: usize, dy: usize| self.data[(2 * y + dy) * self.width + 2 * x + dx];
                    *out = (at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1)) * 0.25;
                }
            });
        Plane {
            width: w,
            height: h,
            data,
        }
    }
}

/// The L* plane of `image` box-averaged over `factor` by `factor` blocks
/// (the RGB is averaged, then converted), dropping partial blocks.
fn luminance(image: &Image, factor: usize, to_lab: fn([f32; 3]) -> [f32; 3]) -> Plane {
    let (w, h) = (image.width() / factor, image.height() / factor);
    let src = image.data();
    let stride = image.width() * CHANNELS;
    let mut data = vec![0.0f32; w * h];
    let norm = 1.0 / (factor * factor) as f32;
    data.par_chunks_mut(w.max(1))
        .enumerate()
        .for_each(|(y, row)| {
            for (x, out) in row.iter_mut().enumerate() {
                let mut sum = [0.0f32; 3];
                for dy in 0..factor {
                    let start = (y * factor + dy) * stride + x * factor * CHANNELS;
                    for px in src[start..start + factor * CHANNELS]
                        .as_chunks::<CHANNELS>()
                        .0
                    {
                        sum[0] += px[0];
                        sum[1] += px[1];
                        sum[2] += px[2];
                    }
                }
                *out = to_lab([sum[0] * norm, sum[1] * norm, sum[2] * norm])[0];
            }
        });
    Plane {
        width: w,
        height: h,
        data,
    }
}

/// A luminance pyramid: `levels[k]` is the image box-reduced by
/// `factors[k]`, finest first.
struct Pyramid {
    levels: Vec<Plane>,
    /// The gradient magnitude of each level.
    gradients: Vec<Plane>,
    factors: Vec<usize>,
}

impl Pyramid {
    /// From `image` down to a long side of at most `coarsest`, starting at
    /// a long side of at most [`FINE_LONG_SIDE`].
    fn build(image: &Image, to_lab: fn([f32; 3]) -> [f32; 3], coarsest: usize) -> Pyramid {
        let long = image.width().max(image.height());
        let first = long.div_ceil(FINE_LONG_SIDE).max(1);
        let mut levels = vec![luminance(image, first, to_lab)];
        let mut factors = vec![first];
        loop {
            let last = &levels[levels.len() - 1];
            if last.width.max(last.height) <= coarsest || last.width.min(last.height) < 8 {
                break;
            }
            let next = last.half();
            levels.push(next);
            factors.push(factors[factors.len() - 1] * 2);
        }
        let gradients = levels.iter().map(Plane::gradient).collect();
        Pyramid {
            levels,
            gradients,
            factors,
        }
    }

    /// The coarsest level whose factor is at most `factor`, or the finest
    /// when none is.
    fn level_for(&self, factor: f64) -> usize {
        let mut best = 0;
        for (k, &f) in self.factors.iter().enumerate() {
            if f as f64 <= factor {
                best = k;
            }
        }
        best
    }
}

/// What a correlation is taken of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Channel {
    /// L*: smooth, with a wide capture range; the search runs on it.
    Luminance,
    /// The gradient magnitude of L*: edges only, blind to the gentle
    /// gradients any two pictures share; the verdict is taken on it.
    Gradient,
}

/// One search candidate.
#[derive(Clone, Copy, Debug)]
struct Candidate {
    orientation: Orientation,
    scale: f64,
    offset: [f64; 2],
    angle_degrees: f64,
    score: f64,
}

/// The two images and their pyramids, with the full-resolution sizes.
struct Search {
    render: Pyramid,
    reference: Pyramid,
    render_size: (usize, usize),
    reference_size: (usize, usize),
}

impl Search {
    /// What the search maximises for the candidate on render level
    /// `level`: the mean of the normalised cross-correlations of L* and of
    /// its gradient magnitude. L* alone has a wide capture range, but on a
    /// dark or smooth scene (a ramp, a vignette) it is nearly the same for
    /// every placement and the few edges decide; the gradient magnitude is
    /// blind to the shading and sees only the edges. `None` when the
    /// candidate breaks a bound of the search (coverage, [`MIN_OVERLAP`],
    /// [`MIN_REFERENCE_INSIDE`]) or either side is flat over the overlap
    /// in either channel.
    fn score(
        &self,
        level: usize,
        orientation: Orientation,
        scale: f64,
        offset: [f64; 2],
        angle_degrees: f64,
    ) -> Option<f64> {
        let tone = self.correlate(
            Channel::Luminance,
            level,
            orientation,
            scale,
            offset,
            angle_degrees,
        )?;
        let plane = &self.render.levels[level];
        if plane.width.max(plane.height) < EDGE_MIN_LONG_SIDE {
            return Some(tone);
        }
        let edges = self.correlate(
            Channel::Gradient,
            level,
            orientation,
            scale,
            offset,
            angle_degrees,
        )?;
        Some(0.5 * (tone + edges))
    }

    /// The normalised cross-correlation of `channel` for the candidate on
    /// render level `level`, under the bounds of [`Search::score`].
    fn correlate(
        &self,
        channel: Channel,
        level: usize,
        orientation: Orientation,
        scale: f64,
        offset: [f64; 2],
        angle_degrees: f64,
    ) -> Option<f64> {
        let (w, h) = (self.render_size.0 as f64, self.render_size.1 as f64);
        let (ow, oh) = orientation.oriented_size(self.reference_size.0, self.reference_size.1);
        let (ext_w, ext_h) = (scale * ow as f64, scale * oh as f64);
        if ext_w < MIN_COVERAGE * w || ext_w > MAX_COVERAGE * w || ext_h > MAX_COVERAGE * h {
            return None;
        }
        let (planes, ref_planes) = match channel {
            Channel::Luminance => (&self.render.levels, &self.reference.levels),
            Channel::Gradient => (&self.render.gradients, &self.reference.gradients),
        };
        let plane = &planes[level];
        let d = self.render.factors[level] as f64;
        let ref_level = self.reference.level_for(d / scale);
        let ref_plane = &ref_planes[ref_level];
        let e = self.reference.factors[ref_level] as f64;
        // Render level pixel -> full-resolution render pixel.
        let level_to_full = Affine::scale_shift(d, (d - 1.0) / 2.0, (d - 1.0) / 2.0);
        // Reference full-resolution pixel -> reference level pixel.
        let full_to_level =
            Affine::scale_shift(1.0 / e, -(e - 1.0) / (2.0 * e), -(e - 1.0) / (2.0 * e));
        let registration = Registration {
            orientation,
            scale,
            offset,
            angle_degrees,
            correlation: 0.0,
            luminance_correlation: 0.0,
            reference_size: self.reference_size,
        };
        let map = full_to_level
            .after(&registration.render_to_reference())
            .after(&level_to_full);

        // The level pixels whose full-resolution centres see the
        // reference: the full-resolution overlap, mapped back.
        let (fx0, fy0, fx1, fy1) =
            registration.overlap_rect(self.render_size.0, self.render_size.1)?;
        if (((fx1 - fx0) * (fy1 - fy0)) as f64) < MIN_REFERENCE_INSIDE * ext_w * ext_h {
            return None;
        }
        let to_level = |v: usize, ceil: bool| {
            let l = (v as f64 - (d - 1.0) / 2.0) / d;
            if ceil { l.ceil() } else { l.floor() }
        };
        let i0 = to_level(fx0, true).max(0.0) as usize;
        let j0 = to_level(fy0, true).max(0.0) as usize;
        let i1 = (to_level(fx1 - 1, false) + 1.0).min(plane.width as f64);
        let j1 = (to_level(fy1 - 1, false) + 1.0).min(plane.height as f64);
        if i1 <= i0 as f64 || j1 <= j0 as f64 {
            return None;
        }
        let (i1, j1) = (i1 as usize, j1 as usize);
        let n = ((i1 - i0) * (j1 - j0)) as f64;
        if n < MIN_OVERLAP * (plane.width * plane.height) as f64 {
            return None;
        }
        let rows: Vec<usize> = (j0..j1).step_by(BAND_ROWS).collect();
        let band = |&start: &usize| {
            let mut s = [0.0f64; 5];
            for j in start..(start + BAND_ROWS).min(j1) {
                let row = &plane.data[j * plane.width..(j + 1) * plane.width];
                for (i, &a) in row.iter().enumerate().take(i1).skip(i0) {
                    let (p, q) = map.apply(i as f64, j as f64);
                    let a = f64::from(a);
                    let b = ref_plane.sample(p, q);
                    s[0] += a;
                    s[1] += b;
                    s[2] += a * a;
                    s[3] += b * b;
                    s[4] += a * b;
                }
            }
            s
        };
        let bands: Vec<[f64; 5]> = if n < PARALLEL_PIXELS {
            rows.iter().map(band).collect()
        } else {
            rows.par_iter().map(band).collect()
        };
        let mut s = [0.0f64; 5];
        for band in &bands {
            for (t, v) in s.iter_mut().zip(band) {
                *t += v;
            }
        }
        let var_a = s[2] - s[0] * s[0] / n;
        let var_b = s[3] - s[1] * s[1] / n;
        let cov = s[4] - s[0] * s[1] / n;
        let floor = 1e-9 * n;
        if var_a <= floor || var_b <= floor {
            return None;
        }
        Some((cov / (var_a * var_b).sqrt()).clamp(-1.0, 1.0))
    }

    /// The exhaustive coarse search of one orientation at one scale: the
    /// best whole-coarse-pixel translation, or `None`.
    fn coarse(&self, orientation: Orientation, scale: f64) -> Option<Candidate> {
        let level = self.render.levels.len() - 1;
        let d = self.render.factors[level] as f64;
        let (w, h) = (self.render_size.0 as f64, self.render_size.1 as f64);
        let (ow, oh) = orientation.oriented_size(self.reference_size.0, self.reference_size.1);
        let range = |n: f64, n_ref: usize| {
            let extent = scale * n_ref as f64;
            let lo = (n - extent).min(0.0) - MARGIN * n;
            let hi = (n - extent).max(0.0) + MARGIN * n;
            (lo, hi)
        };
        let (xlo, xhi) = range(w, ow);
        let (ylo, yhi) = range(h, oh);
        let mut best: Option<Candidate> = None;
        let mut ty = ylo;
        while ty <= yhi {
            let mut tx = xlo;
            while tx <= xhi {
                if let Some(score) =
                    self.correlate(Channel::Luminance, level, orientation, scale, [tx, ty], 0.0)
                    && best.is_none_or(|b| score > b.score)
                {
                    best = Some(Candidate {
                        orientation,
                        scale,
                        offset: [tx, ty],
                        angle_degrees: 0.0,
                        score,
                    });
                }
                tx += d;
            }
            ty += d;
        }
        best
    }

    /// Coordinate hill climb of `c` on render level `level`: steps of one
    /// level pixel in each offset, the scale step that moves the far edge
    /// by one level pixel and the angle step that moves the far corner by
    /// one level pixel (angle kept within [`MAX_ROTATION_DEGREES`]),
    /// halved whenever no step improves, down to `1 / min_fraction` of a
    /// level pixel.
    fn climb(&self, level: usize, c: Candidate, min_fraction: f64) -> Candidate {
        let d = self.render.factors[level] as f64;
        let (ow, oh) = c
            .orientation
            .oriented_size(self.reference_size.0, self.reference_size.1);
        let Some(start) = self.score(level, c.orientation, c.scale, c.offset, c.angle_degrees)
        else {
            return Candidate {
                score: f64::NEG_INFINITY,
                ..c
            };
        };
        let mut best = Candidate { score: start, ..c };
        // On a tiny image a turn of a degree moves the edges by well under
        // a pixel and only trades against the offsets.
        let plane = &self.render.levels[level];
        let rotate = plane.width.max(plane.height) >= ROTATION_MIN_LONG_SIDE;
        let mut step = d;
        let min_step = d / min_fraction;
        for _ in 0..400 {
            let ds = step / ow as f64;
            // Radians per step: one level pixel at the reference's corner.
            let corner = 0.5 * best.scale * (ow as f64).hypot(oh as f64);
            let da = (step / corner).to_degrees();
            let da = if rotate { da } else { 0.0 };
            let moves = [
                (ds, 0.0, 0.0, 0.0),
                (-ds, 0.0, 0.0, 0.0),
                (0.0, step, 0.0, 0.0),
                (0.0, -step, 0.0, 0.0),
                (0.0, 0.0, step, 0.0),
                (0.0, 0.0, -step, 0.0),
                (0.0, 0.0, 0.0, da),
                (0.0, 0.0, 0.0, -da),
            ];
            let mut improved = false;
            for (dsc, dx, dy, dangle) in moves {
                if dsc == 0.0 && dx == 0.0 && dy == 0.0 && dangle == 0.0 {
                    continue;
                }
                let scale = best.scale + dsc;
                let angle = best.angle_degrees + dangle;
                if scale <= 0.0 || angle.abs() > MAX_ROTATION_DEGREES {
                    continue;
                }
                // A scale step keeps the reference's centre where it is,
                // so that it does not trade against the offsets.
                let offset = [
                    best.offset[0] + dx - 0.5 * dsc * (ow as f64 - 1.0),
                    best.offset[1] + dy - 0.5 * dsc * (oh as f64 - 1.0),
                ];
                if let Some(score) = self.score(level, best.orientation, scale, offset, angle)
                    && score > best.score
                {
                    best = Candidate {
                        scale,
                        offset,
                        angle_degrees: angle,
                        score,
                        ..best
                    };
                    improved = true;
                }
            }
            if !improved {
                if step <= min_step {
                    break;
                }
                step /= 2.0;
            }
        }
        best
    }
}

/// Registers `reference` (sRGB-encoded) onto `render` (linear Rec.2020):
/// the orientation, scale and offset with the highest L* correlation over
/// the overlap (see the module documentation). `None` when no candidate
/// covers [`MIN_OVERLAP`] of the render with any texture, for example
/// when either image is flat or empty. Whether the result is good enough
/// is [`Registration::is_aligned`].
pub fn register(render: &Image, reference: &Image) -> Option<Registration> {
    if render.width() < 2 || render.height() < 2 || reference.width() < 2 || reference.height() < 2
    {
        return None;
    }
    let render_pyramid = Pyramid::build(render, rec2020_linear_to_lab, COARSE_LONG_SIDE);
    let reference_pyramid = Pyramid::build(reference, srgb_to_lab, COARSE_LONG_SIDE / 2);
    let search = Search {
        render: render_pyramid,
        reference: reference_pyramid,
        render_size: (render.width(), render.height()),
        reference_size: (reference.width(), reference.height()),
    };
    let coarse_level = search.render.levels.len() - 1;
    let coarse = &search.render.levels[coarse_level];
    // Geometric scale steps that move the far edge by one coarse pixel.
    let ratio = 1.0 + 1.0 / coarse.width.max(1) as f64;
    let steps = ((MAX_COVERAGE / MIN_COVERAGE).ln() / ratio.ln()).ceil() as usize;
    let jobs: Vec<(Orientation, f64)> = ORIENTATIONS
        .iter()
        .flat_map(|&o| {
            let (ow, _) = o.oriented_size(reference.width(), reference.height());
            let base = MIN_COVERAGE * render.width() as f64 / ow as f64;
            (0..=steps).map(move |k| (o, base * ratio.powi(k as i32)))
        })
        .collect();
    let found: Vec<Option<Candidate>> = jobs
        .par_iter()
        .map(|&(o, scale)| search.coarse(o, scale))
        .collect();
    // The best few distinct placements of each orientation, in orientation
    // order (see [`COARSE_SEEDS`]).
    let mut field: Vec<Candidate> = Vec::new();
    for o in ORIENTATIONS {
        let mut mine: Vec<Candidate> = found
            .iter()
            .flatten()
            .filter(|c| c.orientation == o)
            .copied()
            .collect();
        // Stable: ties keep scale order.
        mine.sort_by(|a, b| b.score.total_cmp(&a.score));
        let coarse_px = search.render.factors[coarse_level] as f64;
        let mut kept: Vec<Candidate> = Vec::new();
        for c in mine {
            let distinct = kept.iter().all(|k| {
                (c.offset[0] - k.offset[0]).abs() > SEED_SEPARATION * coarse_px
                    || (c.offset[1] - k.offset[1]).abs() > SEED_SEPARATION * coarse_px
                    || (c.scale / k.scale - 1.0).abs() > SEED_SCALE_SEPARATION
            });
            if distinct {
                kept.push(c);
            }
            if kept.len() == COARSE_SEEDS {
                break;
            }
        }
        field.extend(kept);
    }
    if field.is_empty() {
        return None;
    }
    // The seeds go to the next finer level as they are: a hill climb on
    // the coarsest level would follow its flat ridge away from a seed that
    // is right (see [`COARSE_SEEDS`]). With a single level, climb on it.
    let first = coarse_level.saturating_sub(1);
    // A turn of a degree or two changes the L* correlation only a little,
    // too little for a hill climb from no turn to find on its own, so each
    // seed also starts at a turn either way (ANGLE_SEEDS).
    let first_plane = &search.render.levels[first];
    if first_plane.width.max(first_plane.height) >= ROTATION_MIN_LONG_SIDE {
        field = field
            .iter()
            .flat_map(|&c| {
                ANGLE_SEEDS.iter().map(move |&a| Candidate {
                    angle_degrees: a,
                    ..c
                })
            })
            .collect();
    }
    for level in (0..=first).rev() {
        let min_fraction = if level == 0 { 8.0 } else { 4.0 };
        let climbed: Vec<Candidate> = field
            .par_iter()
            .map(|&c| search.climb(level, c, min_fraction))
            .collect();
        field = climbed
            .into_iter()
            .filter(|c| c.score.is_finite())
            .collect();
        // Stable sort: ties keep orientation order.
        field.sort_by(|a, b| b.score.total_cmp(&a.score));
        // Climbs from neighbouring seeds end on the same placement; keep
        // one of them.
        let d = search.render.factors[level] as f64;
        let mut distinct: Vec<Candidate> = Vec::new();
        for c in field {
            let same = distinct.iter().any(|k| {
                k.orientation == c.orientation
                    && (c.offset[0] - k.offset[0]).abs() <= d
                    && (c.offset[1] - k.offset[1]).abs() <= d
                    && (c.scale / k.scale - 1.0).abs() <= SEED_SCALE_SEPARATION / 4.0
                    && (c.angle_degrees - k.angle_degrees).abs() <= ANGLE_SEPARATION_DEGREES
            });
            if !same {
                distinct.push(c);
            }
        }
        field = distinct;
        field.truncate(if level == first {
            COARSE_SURVIVORS
        } else {
            SURVIVORS
        });
    }
    let best = field.first()?;
    let at = |channel| {
        search.correlate(
            channel,
            0,
            best.orientation,
            best.scale,
            best.offset,
            best.angle_degrees,
        )
    };
    let edges = at(Channel::Gradient)?;
    let tone = at(Channel::Luminance)?;
    Some(Registration {
        orientation: best.orientation,
        scale: best.scale,
        offset: best.offset,
        angle_degrees: best.angle_degrees,
        correlation: edges,
        luminance_correlation: tone,
        reference_size: search.reference_size,
    })
}

/// Bilinear sample of `image` at continuous pixel-index coordinates,
/// clamped to the pixel centres.
pub fn sample_rgb(image: &Image, p: f64, q: f64) -> [f32; 3] {
    let (w, h) = (image.width(), image.height());
    let p = p.clamp(0.0, (w - 1) as f64);
    let q = q.clamp(0.0, (h - 1) as f64);
    let x0 = (p.floor() as usize).min(w.saturating_sub(2));
    let y0 = (q.floor() as usize).min(h.saturating_sub(2));
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);
    let (fx, fy) = ((p - x0 as f64) as f32, (q - y0 as f64) as f32);
    let (a, b, c, d) = (
        image.pixel(x0, y0),
        image.pixel(x1, y0),
        image.pixel(x0, y1),
        image.pixel(x1, y1),
    );
    std::array::from_fn(|k| {
        let top = a[k] * (1.0 - fx) + b[k] * fx;
        let bottom = c[k] * (1.0 - fx) + d[k] * fx;
        top * (1.0 - fy) + bottom * fy
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orientation_maps_match_geometry_apply() {
        // A 3x2 image whose pixel (x, y) holds 10 y + x; every orientation
        // applied with Geometry and read back through to_original gives
        // the same source index.
        let mut img = Image::new(3, 2);
        for y in 0..2 {
            for x in 0..3 {
                img.set_pixel(x, y, [(10 * y + x) as f32; 3]);
            }
        }
        for o in ORIENTATIONS {
            let g = negg_types::Geometry {
                rotate_degrees: o.rotate_degrees,
                flip_horizontal: o.flip_horizontal,
                ..negg_types::Geometry::default()
            };
            let out = g.apply(&img).unwrap();
            let map = o.to_original(3, 2);
            assert_eq!((out.width(), out.height()), o.oriented_size(3, 2));
            for v in 0..out.height() {
                for u in 0..out.width() {
                    let (p, q) = map.apply(u as f64, v as f64);
                    let want = out.pixel(u, v)[0];
                    let got = img.pixel(p.round() as usize, q.round() as usize)[0];
                    assert_eq!(got, want, "{o:?} at ({u}, {v})");
                }
            }
        }
    }

    #[test]
    fn overlap_rect_is_the_covered_render_pixels() {
        let r = Registration {
            orientation: ORIENTATIONS[0],
            scale: 2.0,
            offset: [10.5, -3.0],
            angle_degrees: 0.0,
            correlation: 1.0,
            luminance_correlation: 1.0,
            reference_size: (20, 10),
        };
        // x from 10.5 to 10.5 + 2 * 19 = 48.5: pixels 11..=48; y from -3 to
        // 15: pixels 0..=15.
        assert_eq!(r.overlap_rect(100, 100), Some((11, 0, 49, 16)));
        assert_eq!(r.overlap_rect(30, 8), Some((11, 0, 30, 8)));
        assert!((r.overlap_fraction(100, 100) - 38.0 * 16.0 / 1e4).abs() < 1e-12);
        let far = Registration {
            offset: [200.0, 0.0],
            ..r
        };
        assert_eq!(far.overlap_rect(100, 100), None);
        assert_eq!(far.overlap_fraction(100, 100), 0.0);
    }

    #[test]
    fn flat_images_do_not_register() {
        let mut flat = Image::new(40, 30);
        for y in 0..30 {
            for x in 0..40 {
                flat.set_pixel(x, y, [0.4; 3]);
            }
        }
        assert!(register(&flat, &flat).is_none());
        assert!(register(&Image::new(1, 1), &flat).is_none());
    }
}
