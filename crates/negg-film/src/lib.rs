// SPDX-License-Identifier: MPL-2.0

//! Film profiles, Dmin/Dmax/gamma estimators and roll pooling.
//!
//! Owns per-film and per-roll statistics: the block-median analysis grid of
//! step 3 of `docs/MATHS.md`, the density transform, the Dmin and Dmax
//! estimators, the no-profile gamma of step 4 and the exposure anchor of
//! step 5. Must not do I/O, decode or per-pixel rendering: the step 4
//! inversion lives in `negg-render`, and [`exposure_anchor`] takes the
//! already inverted grid so that this crate depends on `negg-types` only.
//!
//! Every statistic here is robust in the sense of ground rule 5: it is a
//! percentile or a trimmed mean over block medians, never a raw extreme or
//! a plain mean over a rectangle, so dust and scratches do not move it.
//!
//! Phase 2 adds roll pooling ([`pool_channels`], [`pool_cast`]), the patch
//! statistic ([`patch_median`]) and the neutral-patch fit ([`fit_neutral`]).

mod anchor;
mod estimators;
mod grid;
mod neutral;
mod patch;
mod pool;
mod stats;

pub use anchor::exposure_anchor;
pub use estimators::{DminEstimate, estimate_dmax, estimate_dmin, no_profile_gamma};
pub use grid::{
    AnalysisGrid, TRANSMITTANCE_FLOOR, analysis_grid, density, density_value, grid_cells_in,
    sub_grid,
};
pub use neutral::{NEUTRAL_MIN_SPREAD, NeutralFit, fit_neutral};
pub use patch::patch_median;
pub use pool::{
    ChannelPool, ROLL_REJECT_LOG_D, dmin_candidates, pool_cast, pool_channels, weighted_median,
};
pub use stats::{median, percentile, trimmed_mean};
