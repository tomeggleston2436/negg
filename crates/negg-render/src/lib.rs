// SPDX-License-Identifier: MPL-2.0

//! Every per-pixel operation as a pure function on a buffer plus
//! settings: inversion, print curve, adjustments and scopes.
//!
//! Must not do I/O or decode, and must not hold a second copy of maths
//! that lives in another crate.
//!
//! The modules follow the steps of `docs/MATHS.md`:
//!
//! - [`invert`]: step 4, density to normalised log exposure.
//! - [`print`]: step 5, the print stage (grade, exposure anchor, softplus
//!   shoulder and toe, optional black point compensation).
//! - [`cast`]: step 6, auto colour (cast fit on the analysis grid and the
//!   affine correction it applies).
//! - [`matrix`]: step 7, the camera matrix applied to the positive.
//! - [`rig`]: step 2 at render time (flat field gain and crosstalk unmix
//!   on the linear image) and the step 3 density with a rig density model.
//! - [`full`]: all of the above fused into one pass per pixel.
//! - [`pipeline`]: steps 3 to 7 orchestrated for one frame (the analysis
//!   grid and estimators of `negg-film`, the precedence of fixed settings,
//!   film correction and roll pool, then the fused render).
//! - [`roll`]: pass 1 of roll mode (each frame's analysis, then the Dmin,
//!   gamma and cast pools of steps 3 and 6).
//!
//! Every function here is pure: it reads an image and a settings value and
//! writes an image, with rayon splitting the work over row bands.

pub mod cast;
pub mod full;
pub mod invert;
pub mod matrix;
mod par;
pub mod pipeline;
pub mod print;
pub mod rig;
pub mod roll;

pub use cast::{apply_cast, apply_cast_pixel, fit_cast};
pub use full::render_full;
pub use invert::{invert, invert_in_place, invert_pixel};
pub use matrix::{apply_matrix, apply_matrix_pixel};
pub use pipeline::{
    ANALYSIS_GRID_LONG_SIDE, FrameRender, RenderContext, estimate_frame, render_frame,
    render_frame_with,
};
pub use print::{
    ANCHOR_PRINT_DENSITY, PrintCoefficients, grade_k, print, print_pixel, softplus, softplus_inv,
    solve_x0,
};
pub use rig::{apply_rig_linear, density_grid, density_model_pixel};
pub use roll::{FrameAnalysis, RollFrame, analyse_frame, pool_roll};
