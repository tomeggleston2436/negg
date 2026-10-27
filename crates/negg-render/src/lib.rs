// SPDX-License-Identifier: MPL-2.0

//! Every per-pixel operation as a pure function on a buffer plus
//! settings: inversion, print curve, adjustments and scopes.
//!
//! Must not do I/O or decode (the GPU preview kernel of the `gpu` feature
//! is the one exception: it runs this crate's kernel on a GPU device), and
//! must not hold a second copy of maths that lives in another crate.
//!
//! The modules follow the steps of `docs/MATHS.md`:
//!
//! - [`invert`]: step 4, density to normalised log exposure.
//! - [`print`]: step 5, the print stage (grade, exposure anchor, softplus
//!   shoulder and toe, optional black point compensation).
//! - [`cast`]: step 6, auto colour (cast fit on the analysis grid and the
//!   affine correction it applies).
//! - [`matrix`]: step 7, the entry matrix into Rec.2020 applied to the positive.
//! - [`saturation`]: step 7, saturation about the Rec.2020 luminance.
//! - [`rig`]: step 2 at render time (flat field gain and crosstalk unmix
//!   on the linear image) and the step 3 density with a rig density model.
//! - [`full`]: all of the above fused into one pass per pixel.
//! - `gpu` (feature `gpu`): the same fused kernel on the GPU for preview
//!   tiles, tested against [`full`] (ADR 0036).
//! - [`pipeline`]: steps 3 to 7 orchestrated for one frame (the analysis
//!   grid and estimators of `negg-film`, the precedence of fixed settings,
//!   film correction and roll pool, then the fused render).
//! - [`roll`]: pass 1 of roll mode (each frame's analysis, then the Dmin,
//!   gamma and cast pools of steps 3 and 6).
//! - [`mod@pyramid`]: step 8 preview, the linear-light pyramid whose levels
//!   are rendered tile by tile with [`render_region`] and a [`FramePlan`].
//! - [`mod@scopes`]: step 8 preview, the histogram, parade and vectorscope of
//!   an 8-bit sRGB render.
//! - [`negative`]: step 8 preview, the decoded negative shown before
//!   analysis (scaled and clipped, not inverted).
//!
//! Every function here is pure: it reads an image and a settings value and
//! writes an image, with rayon splitting the work over row bands.

pub mod cast;
pub mod full;
#[cfg(feature = "gpu")]
pub mod gpu;
pub mod invert;
pub mod matrix;
pub mod negative;
mod par;
pub mod pipeline;
pub mod print;
pub mod pyramid;
pub mod resample;
pub mod rig;
pub mod roll;
pub mod saturation;
pub mod scopes;

pub use cast::{apply_cast, apply_cast_pixel, fit_cast};
pub use full::{Region, render_full, render_region};
pub use invert::{invert, invert_in_place, invert_pixel};
pub use matrix::{apply_matrix, apply_matrix_pixel};
pub use negative::{
    NEGATIVE_WHITE_PERCENTILE, negative_display, negative_display_pixel, negative_display_scale,
};
pub use pipeline::{
    ANALYSIS_GRID_LONG_SIDE, FramePlan, FrameRender, MIN_ANALYSIS_CELLS, RenderContext,
    estimate_frame, plan_frame, render_frame, render_frame_with,
};
pub use print::{
    ANCHOR_PRINT_DENSITY, PrintCoefficients, grade_k, print, print_pixel, softplus, softplus_inv,
    solve_x0,
};
pub use pyramid::{TILE_SIZE, downscale_half, pyramid};
pub use resample::downscale_to_long_edge;
pub use rig::{apply_rig_linear, density_grid, density_model_pixel};
pub use roll::{FrameAnalysis, RollFrame, analyse_frame, analyse_grid, pool_roll};
pub use saturation::saturate_pixel;
pub use scopes::{SCOPE_BINS, Scopes, scopes};
