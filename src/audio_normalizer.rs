use crate::audio_file::write_mp3;
use anyhow::{Context, Result};
use ebur128::{EbuR128, Mode};
use log::debug;
use std::{
    fs::File,
    path::{Path, PathBuf},
};
use symphonia::{
    core::{
        formats::{FormatOptions, Track},
        io::MediaSourceStream,
        meta::MetadataOptions,
        probe::Hint,
    },
    default,
};

#[derive(Debug)]
pub struct Normalizer {
    target_loudness: f64,
    target_peak: f64,
}

impl Normalizer {
    /// Maximum allowed target loudness is hardcoded to prevent hearing damage
    pub const MAX_TARGET_LOUDNESS: f64 = -15.0;
    /// Maximum allowed peak ceiling is hardcoded to prevent clipping
    pub const MAX_TARGET_PEAK: f64 = -0.1;

    pub const DEFAULT_TARGET_LOUDNESS: f64 = -18.0;
    pub const DEFAULT_TARGET_PEAK: f64 = -1.0;

    pub fn new(target_loudness: f64, target_peak: f64) -> Result<Self> {
        // Ensure values are negative
        if target_loudness >= 0.0 {
            return Err(anyhow::anyhow!(
                "Target loudness must be negative (got: {} LUFS)",
                target_loudness
            ));
        }

        if target_peak >= 0.0 {
            return Err(anyhow::anyhow!(
                "Peak ceiling must be negative (got: {} dBFS)",
                target_peak
            ));
        }

        // Check maximum allowed values
        if target_loudness > Self::MAX_TARGET_LOUDNESS {
            return Err(anyhow::anyhow!(
                "Target loudness `{}` LUFS exceeds maximum allowed value of `{}`",
                target_loudness,
                Self::MAX_TARGET_LOUDNESS
            ));
        }

        if target_peak > Self::MAX_TARGET_PEAK {
            return Err(anyhow::anyhow!(
                "Peak ceiling `{}` dBFS exceeds maximum allowed value of `{}`",
                target_peak,
                Self::MAX_TARGET_PEAK
            ));
        }

        Ok(Self {
            target_loudness,
            target_peak,
        })
    }

    /// Normalize an audio file and save the output as an MP3
    pub fn normalize_file(&self, file: &Path) -> Result<PathBuf> {
        if !file.exists() {
            return Err(anyhow::anyhow!("File not found at: {}", file.display()));
        }

        // Read and process audio
        let (samples, track) = decode_file(&file)?;

        // Get track info
        let channels = track.codec_params.channels.unwrap().count();
        let sample_rate = track.codec_params.sample_rate.unwrap();

        // Measure loudness and apply gain
        let gain = self.measure_loudness(channels, sample_rate, &samples)?;
        let processed_samples = self.apply_gain(&samples, gain)?;

        // Write processed samples to file
        write_mp3(&processed_samples, track, &file)?;

        Ok(file.to_path_buf())
    }

    /// Apply the calculated gain to the audio samples
    ///
    /// This function also limits the gain to the target peak if it is exceeded
    fn apply_gain(&self, samples: &[f32], gain: f64) -> Result<Vec<f32>> {
        // Convert target peak from dB to linear scale
        let peak_limit = db_to_linear(self.target_peak);

        // Find the maximum peak in the input
        let current_peak = max_peak(samples);

        // Calculate the maximum allowed gain to stay under peak ceiling
        let max_gain = peak_limit / current_peak;

        // Use the smaller of the calculated gain and max allowed gain
        let final_gain = gain.min(max_gain);

        debug!(
            "Applying gain: {:.2} dB (limited from {:.2} dB due to peak ceiling)",
            20.0 * final_gain.log10(),
            20.0 * gain.log10()
        );

        // Apply the gain to all samples
        let normalized_samples = samples
            .iter()
            .map(|&s| (s as f64 * final_gain) as f32)
            .collect();

        Ok(normalized_samples)
    }

    /// Measure the loudness of the audio samples
    fn measure_loudness(&self, channels: usize, sample_rate: u32, samples: &[f32]) -> Result<f64> {
        let mut ebu = EbuR128::new(channels as u32, sample_rate, Mode::I | Mode::HISTOGRAM)
            .context("Failed to create EBU R128 analyzer")?;

        ebu.add_frames_f32(samples)
            .context("Failed to analyze audio samples")?;

        let current_loudness = ebu
            .loudness_global()
            .context("Failed to calculate global loudness")?;

        if !current_loudness.is_finite() {
            return Err(anyhow::anyhow!("Invalid loudness value calculated"));
        }

        debug!(
            "Current loudness: {:.1} LUFS, Target: {:.1} LUFS",
            current_loudness, self.target_loudness
        );

        // Calculate gain needed to reach target loudness
        let gain_db = self.target_loudness - current_loudness;
        let linear_gain = db_to_linear(gain_db);

        if !linear_gain.is_finite() {
            return Err(anyhow::anyhow!("Invalid gain value calculated"));
        }

        debug!(
            "Calculated gain: {:.2} dB (linear: {:.4})",
            gain_db, linear_gain
        );

        Ok(linear_gain)
    }
}

/// Convert a linear value to a decibel scale
pub fn linear_to_db(linear: f64) -> f64 {
    20.0 * linear.log10()
}

/// Convert a decibel value to a linear scale
pub fn db_to_linear(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

/// Find the maximum peak in the input
pub fn max_peak(samples: &[f32]) -> f64 {
    samples
        .iter()
        .map(|&s| s.abs() as f64)
        .max_by(|a, b| a.partial_cmp(b).unwrap())
        .unwrap_or(0.0)
}

/// Decode the audio stream to samples
fn decode_to_samples(
    format: &mut Box<dyn symphonia::core::formats::FormatReader>,
    track_id: u32,
    mut decoder: Box<dyn symphonia::core::codecs::Decoder>,
) -> Result<Vec<f32>> {
    let mut samples = Vec::new();
    let mut sample_buf = None;

    while let Ok(packet) = format.next_packet() {
        if packet.track_id() != track_id {
            continue;
        }

        match decoder.decode(&packet) {
            Ok(audio_buf) => {
                // Initialize sample buffer on first decoded packet
                if sample_buf.is_none() {
                    let spec = *audio_buf.spec();
                    let duration = audio_buf.capacity() as u64;
                    sample_buf = Some(symphonia::core::audio::SampleBuffer::<f32>::new(
                        duration, spec,
                    ));
                }

                // Copy decoded audio into interleaved sample buffer
                if let Some(buf) = &mut sample_buf {
                    buf.copy_interleaved_ref(audio_buf);
                    samples.extend_from_slice(buf.samples());
                }
            }
            Err(symphonia::core::errors::Error::DecodeError(_)) => {
                // Skip decode errors and continue
                continue;
            }
            Err(e) => {
                return Err(anyhow::anyhow!("Failed to decode audio: {}", e));
            }
        }
    }

    if samples.is_empty() {
        return Err(anyhow::anyhow!("No samples decoded from audio"));
    }

    Ok(samples)
}

/// Process the audio stream to get samples and track info
fn decode_file(input_path: &Path) -> Result<(Vec<f32>, Track)> {
    // First get the track info
    let file = File::open(input_path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let probe = default::get_probe();
    let format_opts: FormatOptions = Default::default();
    let metadata_opts: MetadataOptions = Default::default();
    let hint = Hint::new();

    let probed = probe.format(&hint, mss, &format_opts, &metadata_opts)?;
    let mut format_reader = probed.format;
    let track = format_reader
        .default_track()
        .context("No default track found")?
        .clone();

    // Get decoder
    let decoder =
        symphonia::default::get_codecs().make(&track.codec_params, &Default::default())?;

    // Decode samples
    let samples = decode_to_samples(&mut format_reader, track.id, decoder)?;

    Ok((samples, track))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_audio_stream() -> Result<()> {
        let wav_path = Path::new("./samples/test.wav");

        // Test WAV file reading
        let (samples, track) = decode_file(&wav_path)?;

        assert!(!samples.is_empty(), "WAV samples should not be empty");
        assert_eq!(track.codec_params.sample_rate.unwrap(), 44100);
        assert!(track.codec_params.channels.unwrap().count() > 0);

        Ok(())
    }

    #[test]
    fn test_normalization_gain_achieves_target() {
        let target_loudness = -15.0;
        let normalizer = Normalizer::new(target_loudness, -1.0).unwrap();

        let wav_path = Path::new("./samples/test.wav");

        // Read the test file
        let (samples, track) = decode_file(wav_path).unwrap();
        let channels = track.codec_params.channels.unwrap().count();
        let sample_rate = track.codec_params.sample_rate.unwrap();

        // Calculate the gain
        let gain = normalizer
            .measure_loudness(channels, sample_rate, &samples)
            .unwrap();

        // Apply the gain to get normalized samples
        let processed_samples = normalizer.apply_gain(&samples, gain).unwrap();

        // Measure the loudness of normalized samples
        let mut ebu =
            EbuR128::new(channels as u32, sample_rate, Mode::I | Mode::TRUE_PEAK).unwrap();
        ebu.add_frames_f32(&processed_samples).unwrap();
        let final_loudness = ebu.loudness_global().unwrap();

        // Assert that the final loudness is within 0.1 LUFS of target
        assert!(
            (final_loudness - target_loudness).abs() < 0.1,
            "Expected loudness {:.1} LUFS, got {:.1} LUFS",
            target_loudness,
            final_loudness
        );
    }

    #[test]
    fn test_decode_to_samples() {
        // Use a real WAV file from samples directory
        let wav_path = Path::new("samples/test.wav");

        println!("Reading test file from: {}", wav_path.display());

        let (samples, track) = decode_file(wav_path).unwrap();
        println!(
            "Track info: channels={}, sample_rate={}",
            track.codec_params.channels.unwrap().count(),
            track.codec_params.sample_rate.unwrap()
        );

        println!("Decoded samples count: {}", samples.len());

        // Basic sanity checks
        assert!(samples.len() > 0, "Should have decoded some samples");
        assert!(
            samples.iter().any(|&x| x != 0.0),
            "Samples should not all be zero"
        );
    }

    #[test]
    fn test_invalid_parameters() {
        // Test exceeding max target loudness
        let result = Normalizer::new(-9.0, -1.0);
        assert!(
            result.is_err(),
            "Should error when target loudness > -10.0 LUFS"
        );
        if let Err(e) = result {
            assert!(
                e.to_string().contains("Target loudness"),
                "Error message should mention target loudness"
            );
        }

        // Test exceeding max peak ceiling
        let result = Normalizer::new(-15.0, 0.0);
        assert!(
            result.is_err(),
            "Should error when peak ceiling > -0.1 dBFS"
        );
        if let Err(e) = result {
            assert!(
                e.to_string().contains("Peak ceiling"),
                "Error message should mention peak ceiling"
            );
        }

        // Test valid parameters
        let result = Normalizer::new(-15.0, -1.0);
        assert!(result.is_ok(), "Should accept valid parameters");
    }

    // Add new test for negative value requirements
    #[test]
    fn test_negative_value_requirements() {
        // Test positive target loudness
        let result = Normalizer::new(1.0, -1.0);
        assert!(
            result.is_err(),
            "Should error when target loudness is positive"
        );
        if let Err(e) = result {
            assert!(
                e.to_string().contains("must be negative"),
                "Error message should mention negative requirement"
            );
        }

        // Test zero target loudness
        let result = Normalizer::new(0.0, -1.0);
        assert!(result.is_err(), "Should error when target loudness is zero");

        // Test positive peak ceiling
        let result = Normalizer::new(-15.0, 1.0);
        assert!(
            result.is_err(),
            "Should error when peak ceiling is positive"
        );
        if let Err(e) = result {
            assert!(
                e.to_string().contains("must be negative"),
                "Error message should mention negative requirement"
            );
        }

        // Test zero peak ceiling
        let result = Normalizer::new(-15.0, 0.0);
        assert!(result.is_err(), "Should error when peak ceiling is zero");

        // Test valid negative values
        let result = Normalizer::new(-15.0, -1.0);
        assert!(result.is_ok(), "Should accept valid negative parameters");
    }
}
