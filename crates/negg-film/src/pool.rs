// SPDX-License-Identifier: MPL-2.0

//! Roll pooling of `docs/MATHS.md` step 3 (R-pool, Ruling 48) and step 6
//! (R-cast-pool, Ruling 49).

use negg_types::{CastCorrection, DminSource, PoolRole};

use crate::stats::median;

/// A frame is rejected from a Dmin or gamma pool when any channel lies more
/// than this many log-D from the pool median (Chebyshev distance). Step 3 of
/// `docs/MATHS.md`, R-pool; Controller choice, `TODO(measure)`.
pub const ROLL_REJECT_LOG_D: f32 = 0.3;

/// Result of [`pool_channels`]: the pooled value and each frame's role.
#[derive(Clone, Debug, PartialEq)]
pub struct ChannelPool {
    /// Per-channel median of the inliers; `None` when there were none.
    pub value: Option<[f32; 3]>,
    /// One role per input frame: Inlier, Rejected or Unused. The caller
    /// re-marks user-fixed frames as Excluded.
    pub roles: Vec<PoolRole>,
}

fn channel_median(values: &[[f32; 3]], members: impl Iterator<Item = usize> + Clone) -> [f32; 3] {
    let mut out = [0.0f32; 3];
    for (c, o) in out.iter_mut().enumerate() {
        let mut v: Vec<f32> = members.clone().map(|i| values[i][c]).collect();
        *o = median(&mut v);
    }
    out
}

/// Pools per-frame three-channel values (Dmin or gamma) over the candidate
/// frames: step 3 of `docs/MATHS.md`, "Roll pooling" (R-pool, Ruling 48).
///
/// Takes the per-channel median over the candidates, rejects any candidate
/// with a channel more than [`ROLL_REJECT_LOG_D`] from it, and pools the
/// per-channel median of the inliers. Non-candidates get
/// [`PoolRole::Unused`]. No candidates, or no inliers, gives `value = None`.
/// Non-finite values are rejected. `candidate` must have the length of
/// `values`; missing entries count as non-candidates.
pub fn pool_channels(values: &[[f32; 3]], candidate: &[bool]) -> ChannelPool {
    let is_cand = |i: usize| {
        candidate.get(i).copied().unwrap_or(false) && values[i].iter().all(|v| v.is_finite())
    };
    let cands: Vec<usize> = (0..values.len()).filter(|&i| is_cand(i)).collect();
    let mut roles: Vec<PoolRole> = (0..values.len())
        .map(|i| {
            if candidate.get(i).copied().unwrap_or(false) {
                PoolRole::Rejected
            } else {
                PoolRole::Unused
            }
        })
        .collect();
    if cands.is_empty() {
        return ChannelPool { value: None, roles };
    }
    let centre = channel_median(values, cands.iter().copied());
    let inliers: Vec<usize> = cands
        .iter()
        .copied()
        .filter(|&i| (0..3).all(|c| (values[i][c] - centre[c]).abs() <= ROLL_REJECT_LOG_D))
        .collect();
    for &i in &inliers {
        roles[i] = PoolRole::Inlier;
    }
    if inliers.is_empty() {
        return ChannelPool { value: None, roles };
    }
    ChannelPool {
        value: Some(channel_median(values, inliers.iter().copied())),
        roles,
    }
}

/// Dmin candidate flags: step 3 of `docs/MATHS.md`, "Roll pooling"
/// (R-pool, Ruling 48).
///
/// The candidates are the non-excluded frames whose source is
/// [`DminSource::Border`] when at least 2 non-excluded frames are Border,
/// else all non-excluded frames. `excluded` shorter than `sources` counts
/// missing entries as not excluded.
pub fn dmin_candidates(sources: &[DminSource], excluded: &[bool]) -> Vec<bool> {
    let free = |i: usize| !excluded.get(i).copied().unwrap_or(false);
    let border = sources
        .iter()
        .enumerate()
        .filter(|&(i, s)| free(i) && *s == DminSource::Border)
        .count();
    sources
        .iter()
        .enumerate()
        .map(|(i, s)| free(i) && (border < 2 || *s == DminSource::Border))
        .collect()
}

/// Weighted median: step 6 of `docs/MATHS.md`, "Cast pooling"
/// (R-cast-pool, Ruling 49).
///
/// Sorts by value ascending (ties keep input order) and returns the first
/// value where the cumulative weight reaches half the total. Entries with a
/// non-positive or non-finite weight, or a non-finite value, are ignored.
/// `None` when the total weight is zero. Lengths must match; extra entries
/// of the longer slice are ignored.
pub fn weighted_median(values: &[f32], weights: &[f32]) -> Option<f32> {
    let mut pairs: Vec<(f32, f64)> = values
        .iter()
        .zip(weights)
        .filter(|&(v, w)| v.is_finite() && w.is_finite() && *w > 0.0)
        .map(|(&v, &w)| (v, f64::from(w)))
        .collect();
    if pairs.is_empty() {
        return None;
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let total: f64 = pairs.iter().map(|p| p.1).sum();
    let mut acc = 0.0;
    for (v, w) in &pairs {
        acc += w;
        if acc >= total / 2.0 {
            return Some(*v);
        }
    }
    pairs.last().map(|p| p.0)
}

/// Pools the per-frame cast corrections: step 6 of `docs/MATHS.md`, "Cast
/// pooling" (R-cast-pool, Ruling 49).
///
/// A frame contributes when `candidate` is true and `cells_used > 0`; its
/// weight is `cells_used`. Each of `slope_r`, `offset_r`, `slope_b` and
/// `offset_b` is the [`weighted_median`] of its own values, and the pooled
/// `cells_used` is the sum of the contributing weights. Contributors get
/// [`PoolRole::Inlier`], all others [`PoolRole::Unused`] (the caller
/// re-marks Excluded or Rejected). No contributors gives `None`.
pub fn pool_cast(
    casts: &[CastCorrection],
    candidate: &[bool],
) -> (Option<CastCorrection>, Vec<PoolRole>) {
    let used: Vec<usize> = (0..casts.len())
        .filter(|&i| candidate.get(i).copied().unwrap_or(false) && casts[i].cells_used > 0)
        .collect();
    let mut roles = vec![PoolRole::Unused; casts.len()];
    for &i in &used {
        roles[i] = PoolRole::Inlier;
    }
    if used.is_empty() {
        return (None, roles);
    }
    let w: Vec<f32> = used.iter().map(|&i| casts[i].cells_used as f32).collect();
    let pick = |f: fn(&CastCorrection) -> f32| {
        let v: Vec<f32> = used.iter().map(|&i| f(&casts[i])).collect();
        weighted_median(&v, &w)
    };
    let pooled = (|| {
        Some(CastCorrection {
            slope_r: pick(|c| c.slope_r)?,
            offset_r: pick(|c| c.offset_r)?,
            slope_b: pick(|c| c.slope_b)?,
            offset_b: pick(|c| c.offset_b)?,
            cells_used: used.iter().map(|&i| casts[i].cells_used).sum(),
        })
    })();
    (pooled, roles)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outlier_frame_at_half_a_log_d_is_rejected() {
        let mut v = vec![[0.2, 0.5, 0.8]; 5];
        v[3] = [0.7, 1.0, 1.3];
        let p = pool_channels(&v, &[true; 5]);
        assert_eq!(p.value, Some([0.2, 0.5, 0.8]));
        assert_eq!(p.roles[3], PoolRole::Rejected);
        assert_eq!(p.roles[0], PoolRole::Inlier);
    }

    #[test]
    fn chebyshev_one_channel_is_enough_to_reject() {
        let mut v = vec![[0.2, 0.5, 0.8]; 5];
        v[1] = [0.2, 0.5, 1.2];
        let p = pool_channels(&v, &[true; 5]);
        assert_eq!(p.roles[1], PoolRole::Rejected);
        assert_eq!(p.value, Some([0.2, 0.5, 0.8]));
    }

    #[test]
    fn within_the_limit_is_kept() {
        let mut v = vec![[0.2, 0.5, 0.8]; 5];
        v[1] = [0.4, 0.5, 0.8];
        let p = pool_channels(&v, &[true; 5]);
        assert_eq!(p.roles[1], PoolRole::Inlier);
    }

    #[test]
    fn non_candidates_are_unused_and_do_not_vote() {
        let v = vec![[0.2; 3], [0.2; 3], [5.0; 3]];
        let p = pool_channels(&v, &[true, true, false]);
        assert_eq!(p.roles[2], PoolRole::Unused);
        assert_eq!(p.value, Some([0.2; 3]));
    }

    #[test]
    fn all_rejected_gives_none() {
        // Two candidates 1.0 apart: median is the midpoint, both 0.5 away.
        let v = vec![[0.0; 3], [1.0; 3]];
        let p = pool_channels(&v, &[true, true]);
        assert_eq!(p.value, None);
        assert_eq!(p.roles, vec![PoolRole::Rejected, PoolRole::Rejected]);
        let none = pool_channels(&v, &[false, false]);
        assert_eq!(none.value, None);
        assert_eq!(none.roles, vec![PoolRole::Unused; 2]);
    }

    #[test]
    fn border_preference_with_two_borders() {
        use DminSource::*;
        let s = [Border, Percentile, Border, Percentile];
        assert_eq!(
            dmin_candidates(&s, &[false; 4]),
            vec![true, false, true, false]
        );
    }

    #[test]
    fn one_border_falls_back_to_all() {
        use DminSource::*;
        let s = [Border, Percentile, Percentile];
        assert_eq!(dmin_candidates(&s, &[false; 3]), vec![true; 3]);
    }

    #[test]
    fn excluded_frames_do_not_count_or_qualify() {
        use DminSource::*;
        let s = [Border, Border, Percentile, Fixed];
        // Border count among non-excluded is 1, so all non-excluded qualify.
        assert_eq!(
            dmin_candidates(&s, &[true, false, false, true]),
            vec![false, true, true, false]
        );
    }

    #[test]
    fn weighted_median_examples() {
        assert_eq!(
            weighted_median(&[1.0, 2.0, 3.0], &[1.0, 1.0, 1.0]),
            Some(2.0)
        );
        // Tie at exactly half: first value reaching half the total.
        assert_eq!(weighted_median(&[1.0, 2.0], &[1.0, 1.0]), Some(1.0));
        assert_eq!(
            weighted_median(&[1.0, 2.0, 3.0], &[1.0, 1.0, 10.0]),
            Some(3.0)
        );
        assert_eq!(
            weighted_median(&[3.0, 1.0, 2.0], &[1.0, 5.0, 1.0]),
            Some(1.0)
        );
        // Zero weights are ignored.
        assert_eq!(weighted_median(&[1.0, 9.0], &[0.0, 2.0]), Some(9.0));
        assert_eq!(weighted_median(&[1.0, 9.0], &[0.0, 0.0]), None);
        assert_eq!(weighted_median(&[], &[]), None);
    }

    fn cast(s: f32, cells: u32) -> CastCorrection {
        CastCorrection {
            slope_r: s,
            offset_r: s - 1.0,
            slope_b: 2.0 - s,
            offset_b: 1.0 - s,
            cells_used: cells,
        }
    }

    #[test]
    fn cast_pool_weights_by_cells_and_sums_cells() {
        let casts = [cast(0.9, 10), cast(1.0, 10), cast(1.1, 100)];
        let (p, roles) = pool_cast(&casts, &[true; 3]);
        let p = p.unwrap();
        assert_eq!(p.slope_r, 1.1);
        assert_eq!(p.slope_b, 0.9);
        assert_eq!(p.cells_used, 120);
        assert_eq!(roles, vec![PoolRole::Inlier; 3]);
    }

    #[test]
    fn cast_pool_skips_empty_and_non_candidates() {
        let casts = [cast(0.9, 0), cast(1.0, 10), cast(1.1, 50)];
        let (p, roles) = pool_cast(&casts, &[true, true, false]);
        assert_eq!(p.unwrap().slope_r, 1.0);
        assert_eq!(roles[0], PoolRole::Unused);
        assert_eq!(roles[2], PoolRole::Unused);
        let (none, _) = pool_cast(&casts, &[true, false, false]);
        assert!(none.is_none());
    }
}
