//! Onset envelope and the millisecond refinement of a beat time.
//!
//! The model reads the audio in 20 ms frames. The refinement re-reads the samples themselves: a
//! 1 ms log-energy difference highlights every attack, and a local peak search moves a beat onto
//! the attack within the window the caller allows.

/// The energy envelope of a signal, at roughly one value per millisecond.
pub struct OnsetEnvelope {
    hop_sec: f64,
    strength: Vec<f32>,
}

impl OnsetEnvelope {
    /// True when the signal has no attack at all (digital silence or a constant tone).
    pub fn is_silent(&self) -> bool {
        self.strength.iter().all(|value| *value <= 0.0)
    }

    /// The offset that moves `time` onto the nearest attack, within `max_ms`.
    pub fn snap(&self, time: f64, max_ms: f64) -> f64 {
        let count = self.strength.len() as isize;
        if count < 3 || self.hop_sec <= 0.0 {
            return 0.0;
        }
        let center = (time / self.hop_sec).round() as isize;
        let radius = ((max_ms / 1000.0) / self.hop_sec).round() as isize;
        let radius = radius.max(1);
        let first = (center - radius).max(1);
        let last = (center + radius).min(count - 2);
        if last < first {
            return 0.0;
        }
        let mut best = first;
        let mut peak = self.strength[best as usize];
        for index in (first + 1)..=last {
            let value = self.strength[index as usize];
            if value > peak {
                best = index;
                peak = value;
            }
        }
        if peak <= 0.0 {
            return 0.0;
        }
        let left = self.strength[(best - 1) as usize] as f64;
        let middle = peak as f64;
        let right = self.strength[(best + 1) as usize] as f64;
        let denominator = left - 2.0 * middle + right;
        let offset = if denominator.abs() < 1e-9 {
            0.0
        } else {
            (0.5 * (left - right) / denominator).clamp(-0.5, 0.5)
        };
        (best as f64 + offset) * self.hop_sec - time
    }
}

/// The onset envelope of `samples`, at about one value per millisecond.
pub fn compute(samples: &[f32], sample_rate: u32) -> OnsetEnvelope {
    if samples.is_empty() || sample_rate == 0 {
        return OnsetEnvelope {
            hop_sec: 0.001,
            strength: Vec::new(),
        };
    }
    let hop_samples = ((sample_rate as f64 * 0.001).round() as usize).max(1);
    let hop_sec = hop_samples as f64 / sample_rate as f64;
    let mut energy: Vec<f32> = Vec::with_capacity(samples.len() / hop_samples);
    for frame in samples.chunks_exact(hop_samples) {
        let sum: f32 = frame.iter().map(|sample| sample * sample).sum();
        energy.push(sum / hop_samples as f32);
    }
    let mut strength: Vec<f32> = Vec::with_capacity(energy.len());
    strength.push(0.0);
    for index in 1..energy.len() {
        let previous = (energy[index - 1] + 1e-9).ln();
        let current = (energy[index] + 1e-9).ln();
        strength.push((current - previous).max(0.0));
    }
    OnsetEnvelope { hop_sec, strength }
}

#[cfg(test)]
impl OnsetEnvelope {
    /// An envelope built from explicit values, so a test can place attacks without rendering
    /// audio. The values have the same meaning as the ones `compute` produces.
    pub fn from_strength(hop_sec: f64, strength: Vec<f32>) -> Self {
        OnsetEnvelope { hop_sec, strength }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clicks(sample_rate: u32, times: &[f64]) -> Vec<f32> {
        let mut samples = vec![0.0f32; (sample_rate as f64 * 2.0) as usize];
        let burst = sample_rate as usize / 200;
        for time in times {
            let start = (time * sample_rate as f64) as usize;
            for offset in 0..burst {
                let index = start + offset;
                if index >= samples.len() {
                    break;
                }
                let decay = (-(offset as f32) / (sample_rate as f32 * 0.0015)).exp();
                samples[index] += 0.8 * decay;
            }
        }
        samples
    }

    #[test]
    fn a_beat_snaps_onto_a_nearby_attack() {
        let rate = 48_000;
        let times = [0.5, 1.0, 1.5];
        let samples = clicks(rate, &times);
        let envelope = compute(&samples, rate);
        assert!(!envelope.is_silent());
        for time in times {
            let start = time + 0.004;
            let shifted = start + envelope.snap(start, 30.0);
            assert!(
                (shifted - time).abs() < 0.002,
                "shifted {shifted} from {time}"
            );
        }
    }

    #[test]
    fn silence_reports_no_shift() {
        let samples = vec![0.0f32; 48_000];
        let envelope = compute(&samples, 48_000);
        assert!(envelope.is_silent());
        assert!(envelope.snap(0.5, 30.0).abs() < 1e-12);
    }

    #[test]
    fn an_empty_signal_is_silent() {
        let envelope = compute(&[], 44_100);
        assert!(envelope.is_silent());
        assert!(envelope.snap(0.0, 30.0).abs() < 1e-12);
    }
}
