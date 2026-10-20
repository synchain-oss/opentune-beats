//! Small numeric helpers: medians, percentiles and the least-squares grid fit.

use std::cmp::Ordering;

/// The arithmetic mean, or `0` for an empty slice.
pub fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let sum: f64 = values.iter().sum();
    sum / values.len() as f64
}

/// The median of an unsorted slice, or `0` for an empty slice.
pub fn median(values: &[f64]) -> f64 {
    percentile(values, 0.5)
}

/// The `fraction` quantile of an unsorted slice, linearly interpolated.
pub fn percentile(values: &[f64], fraction: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|left, right| left.partial_cmp(right).unwrap_or(Ordering::Equal));
    if sorted.len() == 1 {
        return sorted[0];
    }
    let position = fraction.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let low = position.floor() as usize;
    let high = position.ceil() as usize;
    if low == high {
        return sorted[low];
    }
    let weight = position - low as f64;
    sorted[low] * (1.0 - weight) + sorted[high] * weight
}

/// The median of `(value, weight)` pairs, or `0` when the input is empty.
pub fn weighted_median(values: &[(f64, f64)]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|left, right| left.0.partial_cmp(&right.0).unwrap_or(Ordering::Equal));
    let total: f64 = sorted.iter().map(|pair| pair.1).sum();
    if total <= 0.0 {
        return sorted[sorted.len() / 2].0;
    }
    let mut seen = 0.0;
    for pair in &sorted {
        seen += pair.1;
        if seen >= total / 2.0 {
            return pair.0;
        }
    }
    sorted[sorted.len() - 1].0
}

/// Least squares `time = offset + index * period` for beats at explicit grid indices.
///
/// The index of a beat is not always its position in the slice: when the model reports no beat
/// during a breakdown, the beat after the gap is several periods later. The caller passes the
/// grid index it computed for every beat, so one missing beat does not bend the fitted tempo.
pub fn fit_indexed(indices: &[f64], times: &[f64]) -> Option<(f64, f64)> {
    if indices.len() != times.len() || indices.len() < 2 {
        return None;
    }
    let mean_index = mean(indices);
    let mean_time = mean(times);
    let mut covariance = 0.0;
    let mut variance = 0.0;
    for (index, time) in indices.iter().zip(times.iter()) {
        let centered = index - mean_index;
        covariance += centered * (time - mean_time);
        variance += centered * centered;
    }
    if variance == 0.0 {
        return None;
    }
    let period = covariance / variance;
    if !period.is_finite() || period <= 0.0 {
        return None;
    }
    Some((mean_time - period * mean_index, period))
}

/// The absolute distance of each beat from the fitted grid, in milliseconds.
pub fn residuals_at_ms(indices: &[f64], times: &[f64], offset: f64, period: f64) -> Vec<f64> {
    let mut out = Vec::with_capacity(times.len());
    for (index, time) in indices.iter().zip(times.iter()) {
        let grid = offset + index * period;
        out.push((time - grid).abs() * 1000.0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_ignores_the_order() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median(&[4.0, 1.0, 3.0, 2.0]), 2.5);
        assert_eq!(median(&[]), 0.0);
    }

    #[test]
    fn percentile_interpolates() {
        assert_eq!(percentile(&[1.0, 2.0, 3.0, 4.0, 5.0], 0.0), 1.0);
        assert_eq!(percentile(&[1.0, 2.0, 3.0, 4.0, 5.0], 1.0), 5.0);
        assert!((percentile(&[1.0, 2.0, 3.0, 4.0, 5.0], 0.5) - 3.0).abs() < 1e-9);
    }

    #[test]
    fn weighted_median_follows_the_weights() {
        let values = [(60.0, 1.0), (120.0, 9.0)];
        assert_eq!(weighted_median(&values), 120.0);
    }

    #[test]
    fn fit_recovers_a_perfect_grid() {
        let times: Vec<f64> = (0..8).map(|index| 0.25 + index as f64 * 0.5).collect();
        let indices: Vec<f64> = (0..8).map(|index| index as f64).collect();
        let (offset, period) = fit_indexed(&indices, &times).expect("fits");
        assert!((offset - 0.25).abs() < 1e-12);
        assert!((period - 0.5).abs() < 1e-12);
        let residuals = residuals_at_ms(&indices, &times, offset, period);
        assert!(residuals.iter().all(|value| *value < 1e-6));
    }

    #[test]
    fn fit_follows_the_grid_indices_across_a_gap() {
        // Four beats at 0.5 s, a ten-beat gap, then four more at 0.5 s.
        let mut indices = vec![0.0, 1.0, 2.0, 3.0];
        let mut times: Vec<f64> = (0..4).map(|index| 0.5 + index as f64 * 0.5).collect();
        for step in 4..14 {
            indices.push(step as f64);
            times.push(0.5 + step as f64 * 0.5);
        }
        let (offset, period) = fit_indexed(&indices, &times).expect("fits");
        assert!((period - 0.5).abs() < 1e-12, "period {period}");
        let residuals = residuals_at_ms(&indices, &times, offset, period);
        assert!(residuals.iter().all(|value| *value < 1e-6));
    }

    #[test]
    fn fit_needs_two_beats_and_a_rising_grid() {
        assert!(fit_indexed(&[0.0], &[1.0]).is_none());
        assert!(fit_indexed(&[0.0, 1.0], &[1.0, 1.0]).is_none());
        assert!(fit_indexed(&[0.0], &[1.0, 2.0]).is_none());
    }
}
