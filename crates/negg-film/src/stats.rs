// SPDX-License-Identifier: MPL-2.0

//! The small robust statistics every estimator in this crate is built from:
//! nearest-rank percentiles, the median and the trimmed mean.
//!
//! All three sort their input in place. They are called on grid-sized
//! slices (hundreds of thousands of cells at most), so an `O(n log n)` sort
//! is cheap and avoids the subtleties of a selection algorithm.

/// Sorts `values` ascending. `NaN` sorts after every number under
/// [`f32::total_cmp`], so a stray `NaN` lands at the top end rather than
/// poisoning the order.
fn sort(values: &mut [f32]) {
    values.sort_by(f32::total_cmp);
}

/// The nearest-rank percentile `p` (in `[0, 100]`) of `values`, which are
/// sorted in place. Used for the 0.5th and 99.5th percentiles of step 3 of
/// `docs/MATHS.md` (`docs/MATHS.md#step-3-density-and-film-base`) and the
/// luma band boundaries of steps 5 and 6.
///
/// The nearest-rank definition returns an element of the input, never an
/// interpolated value: with `n` values the result is the element of rank
/// `ceil(p / 100 * n)`, clamped to `[1, n]`, so `p = 0` is the minimum and
/// `p = 100` the maximum. `p` outside `[0, 100]` is clamped.
///
/// # Panics
///
/// Panics if `values` is empty; a percentile of nothing is not defined.
pub fn percentile(values: &mut [f32], p: f32) -> f32 {
    assert!(!values.is_empty(), "percentile of an empty slice");
    sort(values);
    let n = values.len();
    let p = f64::from(p.clamp(0.0, 100.0));
    // `n` is a slice length, so it converts to f64 exactly for any image
    // this crate will see, and the ceiling is at most `n`.
    let rank = ((p / 100.0) * n as f64).ceil() as usize;
    values[rank.clamp(1, n) - 1]
}

/// The median of `values`, which are sorted in place: the middle element
/// for an odd count, the mean of the two middle elements for an even count.
/// This is the block statistic of the analysis grid of step 3 of
/// `docs/MATHS.md` (`docs/MATHS.md#step-3-density-and-film-base`) and the
/// fallback exposure anchor of step 5.
///
/// # Panics
///
/// Panics if `values` is empty.
pub fn median(values: &mut [f32]) -> f32 {
    assert!(!values.is_empty(), "median of an empty slice");
    sort(values);
    let n = values.len();
    if n % 2 == 1 {
        values[n / 2]
    } else {
        // Averaged in f64 so that two large nearly equal values do not lose
        // a bit; the result is cast back once.
        ((f64::from(values[n / 2 - 1]) + f64::from(values[n / 2])) / 2.0) as f32
    }
}

/// The trimmed mean of `values`, which are sorted in place: drops
/// `floor(n * trim_fraction)` values from each end and returns the mean of
/// what remains. Used for the exposure anchor of step 5 of `docs/MATHS.md`
/// (`docs/MATHS.md#step-5-print-stage-the-tone-curve`) with a trim of 10%.
///
/// `trim_fraction` is clamped to `[0, 0.5]`. If trimming would remove every
/// value (which needs `trim_fraction = 0.5` and an even count) the mean of
/// all the values is returned instead, so the result is always finite for
/// finite input.
///
/// # Panics
///
/// Panics if `values` is empty.
pub fn trimmed_mean(values: &mut [f32], trim_fraction: f32) -> f32 {
    assert!(!values.is_empty(), "trimmed mean of an empty slice");
    sort(values);
    let n = values.len();
    let f = f64::from(trim_fraction.clamp(0.0, 0.5));
    let drop = (f * n as f64).floor() as usize;
    let kept = if 2 * drop < n {
        &values[drop..n - drop]
    } else {
        &values[..]
    };
    let sum: f64 = kept.iter().map(|&v| f64::from(v)).sum();
    (sum / kept.len() as f64) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn percentile_nearest_rank_on_small_arrays() {
        let mut v = [5.0, 1.0, 4.0, 2.0, 3.0];
        assert_eq!(percentile(&mut v, 0.0), 1.0);
        assert_eq!(v, [1.0, 2.0, 3.0, 4.0, 5.0], "sorted in place");
        assert_eq!(percentile(&mut v, 100.0), 5.0);
        // rank = ceil(0.5 * 5) = 3 -> third element
        assert_eq!(percentile(&mut v, 50.0), 3.0);
        // rank = ceil(0.4 * 5) = 2
        assert_eq!(percentile(&mut v, 40.0), 2.0);
        // rank = ceil(0.41 * 5) = 3
        assert_eq!(percentile(&mut v, 41.0), 3.0);
        // tiny percentiles round up to rank 1
        assert_eq!(percentile(&mut v, 0.5), 1.0);
        assert_eq!(percentile(&mut v, 99.5), 5.0);
        // out of range is clamped
        assert_eq!(percentile(&mut v, -10.0), 1.0);
        assert_eq!(percentile(&mut v, 250.0), 5.0);
    }

    #[test]
    fn percentile_of_single_value_is_that_value() {
        let mut v = [7.5];
        assert_eq!(percentile(&mut v, 0.0), 7.5);
        assert_eq!(percentile(&mut v, 50.0), 7.5);
        assert_eq!(percentile(&mut v, 100.0), 7.5);
    }

    #[test]
    fn percentile_half_on_two_hundred_values_is_the_first() {
        let mut v: Vec<f32> = (0..200).map(|i| i as f32).collect();
        // rank = ceil(0.005 * 200) = 1
        assert_eq!(percentile(&mut v, 0.5), 0.0);
        // rank = ceil(0.995 * 200) = 199
        assert_eq!(percentile(&mut v, 99.5), 198.0);
    }

    #[test]
    fn median_odd_and_even() {
        assert_eq!(median(&mut [3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median(&mut [4.0, 1.0, 3.0, 2.0]), 2.5);
        assert_eq!(median(&mut [9.0]), 9.0);
    }

    #[test]
    fn trimmed_mean_drops_floor_of_n_times_f_from_each_end() {
        // n = 10, f = 0.1 -> drop 1 from each end -> mean of 2..=9 = 5.5
        let mut v: Vec<f32> = (1..=10).map(|i| i as f32).collect();
        assert_eq!(trimmed_mean(&mut v, 0.1), 5.5);
        // outliers at both ends are discarded
        let mut v = [100.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, -100.0];
        assert_eq!(trimmed_mean(&mut v, 0.1), 4.5);
        // n = 9, f = 0.1 -> floor(0.9) = 0 -> plain mean
        let mut v: Vec<f32> = (1..=9).map(|i| i as f32).collect();
        assert_eq!(trimmed_mean(&mut v, 0.1), 5.0);
        // f = 0 is the mean
        let mut v = [1.0, 2.0, 6.0];
        assert_eq!(trimmed_mean(&mut v, 0.0), 3.0);
    }

    #[test]
    fn trimmed_mean_that_would_drop_everything_falls_back_to_the_mean() {
        let mut v = [1.0, 3.0];
        assert_eq!(trimmed_mean(&mut v, 0.5), 2.0);
        let mut v = [1.0, 2.0, 6.0];
        // n = 3, f = 0.5 -> drop 1 each end -> middle element
        assert_eq!(trimmed_mean(&mut v, 0.5), 2.0);
        // fractions above 0.5 are clamped
        let mut v = [1.0, 2.0, 6.0];
        assert_eq!(trimmed_mean(&mut v, 0.9), 2.0);
    }

    #[test]
    #[should_panic(expected = "empty")]
    fn percentile_of_empty_panics() {
        let _ = percentile(&mut [], 50.0);
    }

    #[test]
    #[should_panic(expected = "empty")]
    fn trimmed_mean_of_empty_panics() {
        let _ = trimmed_mean(&mut [], 0.1);
    }

    proptest! {
        #[test]
        fn percentile_is_an_element_between_min_and_max(
            values in proptest::collection::vec(-1e3f32..1e3, 1..64),
            p in 0f32..=100.0,
        ) {
            let mut v = values.clone();
            let got = percentile(&mut v, p);
            prop_assert!(values.contains(&got));
            let lo = percentile(&mut v, 0.0);
            let hi = percentile(&mut v, 100.0);
            prop_assert!(lo <= got && got <= hi);
            let min = values.iter().copied().fold(f32::INFINITY, f32::min);
            let max = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            prop_assert_eq!(lo, min);
            prop_assert_eq!(hi, max);
        }

        #[test]
        fn trimmed_mean_lies_within_the_range(
            values in proptest::collection::vec(-1e3f32..1e3, 1..64),
            f in 0f32..=0.5,
        ) {
            let mut v = values.clone();
            let got = trimmed_mean(&mut v, f);
            let min = values.iter().copied().fold(f32::INFINITY, f32::min);
            let max = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            prop_assert!(got.is_finite());
            prop_assert!(min - 1e-3 <= got && got <= max + 1e-3, "{min} {got} {max}");
        }
    }
}
