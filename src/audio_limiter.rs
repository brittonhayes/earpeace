use anyhow::Error;
use log::debug;

use crate::dsp::{db_to_linear, AudioProcessor};

pub struct Limiter {
    threshold: f64,
    release_time: f64,
    lookahead: usize,
}

impl Default for Limiter {
    fn default() -> Self {
        Self {
            threshold: Self::DEFAULT_THRESHOLD,
            release_time: Self::DEFAULT_RELEASE_TIME,
            lookahead: Self::DEFAULT_LOOKAHEAD_MS,
        }
    }
}

impl Limiter {
    pub const DEFAULT_THRESHOLD: f64 = -1.0;
    pub const DEFAULT_RELEASE_TIME: f64 = 50.0; // ms
    pub const DEFAULT_LOOKAHEAD_MS: usize = 5; // ms
    pub const MAX_THRESHOLD: f64 = -0.1;

    pub fn new(threshold: f64, release_time: f64, lookahead_ms: usize) -> Result<Self, Error> {
        // Validate parameters
        if threshold >= 0.0 {
            return Err(anyhow::anyhow!(
                "Threshold must be negative (got: {} dB)",
                threshold
            ));
        }

        if threshold > Self::MAX_THRESHOLD {
            return Err(anyhow::anyhow!(
                "Threshold {} dB exceeds maximum allowed value of {} dB",
                threshold,
                Self::MAX_THRESHOLD
            ));
        }

        if release_time <= 0.0 {
            return Err(anyhow::anyhow!(
                "Release time must be positive (got: {} ms)",
                release_time
            ));
        }

        if lookahead_ms == 0 {
            return Err(anyhow::anyhow!("Lookahead must be greater than 0ms"));
        }

        Ok(Self {
            threshold,
            release_time,
            lookahead: lookahead_ms,
        })
    }
}

impl AudioProcessor for Limiter {
    fn process(
        &self,
        samples: &[f32],
        _channels: usize,
        sample_rate: u32,
    ) -> Result<Vec<f32>, Error> {
        let threshold_linear = db_to_linear(self.threshold);
        let release_samples = (self.release_time * 0.001 * sample_rate as f64) as usize;
        let lookahead_samples = (self.lookahead as f64 * 0.001 * sample_rate as f64) as usize;

        debug!(
            "Limiting with threshold: {:.1} dB, release: {:.1} ms, lookahead: {} ms",
            self.threshold, self.release_time, self.lookahead
        );

        let mut output = vec![0.0; samples.len()];
        let mut gain_reduction = vec![1.0_f32; samples.len()];

        // First pass: calculate gain reduction
        for i in 0..samples.len() {
            let sample_abs = samples[i].abs() as f64;
            if sample_abs > threshold_linear {
                let reduction = (threshold_linear / sample_abs) as f32;
                // Look ahead and apply the reduction
                for j in 0..lookahead_samples {
                    if i + j < gain_reduction.len() {
                        gain_reduction[i + j] = gain_reduction[i + j].min(reduction);
                    }
                }
            }
        }

        // Second pass: smooth gain reduction with release time
        let release_coeff = (-1.0 / (release_samples as f64)).exp() as f32;
        let mut current_reduction = 1.0_f32;

        for i in 0..samples.len() {
            let target_reduction = gain_reduction[i];
            if target_reduction < current_reduction {
                current_reduction = target_reduction;
            } else {
                current_reduction =
                    target_reduction + (current_reduction - target_reduction) * release_coeff;
            }
            output[i] = samples[i] * current_reduction;
        }

        Ok(output)
    }
}
