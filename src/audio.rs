use anyhow::{Context, Result};
use ebur128::{EbuR128, Mode};
use log::debug;
use mp3lame_encoder::{Bitrate, Builder, DualPcm, FlushNoGap, Quality};
use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
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

// Constants
const I16_RANGE: (f32, f32) = (-32768.0, 32767.0);

#[derive(Debug)]
pub struct AudioNormalizer {
    target_loudness: f64,
    peak_ceiling: f64,
}

impl AudioNormalizer {
    pub fn new(target_loudness: f64, peak_ceiling: f64) -> Self {
        Self {
            target_loudness,
            peak_ceiling,
        }
    }

    pub fn normalize_file(&self, input_path: &Path) -> Result<PathBuf> {
        let working_path = self.prepare_working_file(input_path)?;

        // Read and process audio
        let (samples, track) = self.process_audio_stream(&working_path)?;
        let gain = self.calculate_normalization_gain(&track, &samples)?;
        let normalized_samples = self.apply_gain(&samples, gain)?;

        // Write normalized audio
        let output_path = self.create_output_path(input_path);
        self.write_mp3(&output_path, &normalized_samples, track)?;

        // Cleanup temporary files
        if working_path != input_path {
            std::fs::remove_file(working_path)?;
        }

        Ok(output_path)
    }

    fn prepare_working_file(&self, input_path: &Path) -> Result<PathBuf> {
        if !self.is_opus_file(input_path) {
            return Ok(input_path.to_path_buf());
        }

        let temp_mp3 = input_path.with_extension("mp3");
        AudioConverter::convert_opus_to_mp3(input_path, &temp_mp3)?;

        if !temp_mp3.exists() {
            return Err(anyhow::anyhow!(
                "Working file not found at: {}",
                temp_mp3.display()
            ));
        }

        Ok(temp_mp3)
    }

    fn is_opus_file(&self, path: &Path) -> bool {
        path.extension().map_or(false, |ext| ext == "ogg")
    }

    fn create_output_path(&self, input_path: &Path) -> PathBuf {
        input_path.with_file_name(format!(
            "{}-normalized.mp3",
            input_path.file_stem().unwrap().to_string_lossy()
        ))
    }

    fn calculate_normalization_gain(&self, track: &Track, samples: &[f32]) -> Result<f64> {
        let channels = track.codec_params.channels.unwrap().count();
        let sample_rate = track.codec_params.sample_rate.unwrap();
        self.measure_loudness(channels, sample_rate, samples)
    }

    fn apply_gain(&self, samples: &[f32], gain: f64) -> Result<Vec<f32>> {
        // Apply peak ceiling limit to gain
        let peak_gain = 10f64.powf(self.peak_ceiling / 20.0);
        let final_gain = gain.min(peak_gain);

        // Apply normalization with peak ceiling
        let normalized_samples = samples
            .iter()
            .map(|&s| (s as f64 * final_gain) as f32)
            .collect();

        Ok(normalized_samples)
    }

    fn measure_loudness(&self, channels: usize, sample_rate: u32, samples: &[f32]) -> Result<f64> {
        let mut ebu = EbuR128::new(channels as u32, sample_rate, Mode::I | Mode::TRUE_PEAK)
            .context("Failed to create EBU R128 analyzer")?;

        ebu.add_frames_f32(samples)
            .context("Failed to analyze audio samples")?;

        let current_loudness = ebu
            .loudness_global()
            .context("Failed to calculate global loudness")?;

        if !current_loudness.is_finite() {
            return Err(anyhow::anyhow!("Invalid loudness value calculated"));
        }

        let gain_adjustment = if current_loudness < self.target_loudness {
            // Current is quieter than target, increase gain
            debug!(
                "Current loudness {:.1} LUFS is quieter than target {:.1} LUFS, increasing gain",
                current_loudness, self.target_loudness
            );
            self.target_loudness - current_loudness
        } else {
            // Current is louder than target, decrease gain
            debug!(
                "Current loudness {:.1} LUFS is louder than target {:.1} LUFS, decreasing gain",
                current_loudness, self.target_loudness
            );
            self.target_loudness - current_loudness
        };

        let linear_gain = 10f64.powf(gain_adjustment / 20.0);
        if !linear_gain.is_finite() {
            return Err(anyhow::anyhow!("Invalid gain value calculated"));
        }

        Ok(linear_gain)
    }

    fn decode_to_samples(
        &self,
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

    fn write_mp3(
        &self,
        output_path: &Path,
        normalized_samples: &[f32],
        track: Track,
    ) -> Result<()> {
        let channels = track.codec_params.channels.unwrap().count();
        let sample_rate = track.codec_params.sample_rate.unwrap();

        // Create and configure MP3 encoder
        let mut encoder = Builder::new().context("Failed to create LAME builder")?;
        let _ = encoder.set_num_channels(channels as u8);
        let _ = encoder.set_sample_rate(sample_rate);
        let _ = encoder.set_brate(Bitrate::Kbps192);
        let _ = encoder.set_quality(Quality::Good);

        let mut encoder = encoder.build().unwrap();

        // Convert f32 samples to i16 and split into channels
        let samples_i16: Vec<i16> = normalized_samples
            .iter()
            .map(|&x| (x * I16_RANGE.1).clamp(I16_RANGE.0, I16_RANGE.1) as i16)
            .collect();

        // Split samples into left and right channels
        let (left, right) = if channels == 2 {
            let mut left = Vec::with_capacity(samples_i16.len() / 2);
            let mut right = Vec::with_capacity(samples_i16.len() / 2);
            for chunk in samples_i16.chunks(2) {
                left.push(chunk[0]);
                right.push(if chunk.len() > 1 { chunk[1] } else { chunk[0] });
            }
            (left, right)
        } else {
            // Mono: duplicate the same channel
            (samples_i16.clone(), samples_i16)
        };

        let mut output_file =
            File::create(output_path).context("Failed to create output MP3 file")?;
        let mut mp3_buffer =
            vec![std::mem::MaybeUninit::uninit(); mp3lame_encoder::max_required_buffer_size(1024)];

        // Encode chunks
        let chunk_size = 1024; // Process 1024 samples at a time
        for (left_chunk, right_chunk) in left.chunks(chunk_size).zip(right.chunks(chunk_size)) {
            let input = DualPcm {
                left: left_chunk,
                right: right_chunk,
            };

            let encoded = encoder.encode(input, &mut mp3_buffer).unwrap();
            output_file.write_all(unsafe {
                std::slice::from_raw_parts(mp3_buffer.as_ptr() as *const u8, encoded)
            })?;
        }

        // Flush encoder
        let final_bytes = encoder.flush::<FlushNoGap>(&mut mp3_buffer).unwrap();
        output_file.write_all(unsafe {
            std::slice::from_raw_parts(mp3_buffer.as_ptr() as *const u8, final_bytes)
        })?;

        debug!("Wrote normalized MP3 to: {}", output_path.display());
        Ok(())
    }

    fn process_audio_stream(&self, input_path: &Path) -> Result<(Vec<f32>, Track)> {
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
        let samples = self.decode_to_samples(&mut format_reader, track.id, decoder)?;

        Ok((samples, track))
    }
}

pub struct AudioConverter;

impl AudioConverter {
    pub fn convert_opus_to_mp3(input_path: &Path, output_path: &Path) -> Result<()> {
        debug!(
            "Converting Opus to MP3: {} -> {}",
            input_path.display(),
            output_path.display()
        );

        let status = Command::new("ffmpeg")
            .args([
                "-i",
                &input_path.to_string_lossy(),
                "-c:a",
                "libmp3lame",
                "-q:a",
                "2",
                "-y",
                &output_path.to_string_lossy(),
                "-loglevel",
                "quiet", // Suppress ffmpeg output
            ])
            .status()
            .context("Failed to execute ffmpeg")?;

        if !status.success() {
            return Err(anyhow::anyhow!("ffmpeg conversion failed"));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_audio_stream() -> Result<()> {
        let normalizer = AudioNormalizer::new(-14.0, -1.0);
        let wav_path = Path::new("./samples/test.wav");

        // Test WAV file reading
        let (samples, track) = normalizer.process_audio_stream(&wav_path)?;

        assert!(!samples.is_empty(), "WAV samples should not be empty");
        assert_eq!(track.codec_params.sample_rate.unwrap(), 44100);
        assert!(track.codec_params.channels.unwrap().count() > 0);

        Ok(())
    }

    #[test]
    fn test_normalization_gain_achieves_target() -> Result<()> {
        let target_loudness = -14.0;
        let normalizer = AudioNormalizer::new(target_loudness, -1.0);
        let wav_path = Path::new("./samples/test.wav");

        // Read the test file
        let (samples, track) = normalizer.process_audio_stream(wav_path)?;
        let channels = track.codec_params.channels.unwrap().count();
        let sample_rate = track.codec_params.sample_rate.unwrap();

        // Calculate the gain
        let gain = normalizer.measure_loudness(channels, sample_rate, &samples)?;

        // Apply the gain to get normalized samples
        let normalized_samples = normalizer.apply_gain(&samples, gain)?;

        // Measure the loudness of normalized samples
        let mut ebu = EbuR128::new(channels as u32, sample_rate, Mode::I | Mode::TRUE_PEAK)?;
        ebu.add_frames_f32(&normalized_samples)?;
        let final_loudness = ebu.loudness_global()?;

        // Assert that the final loudness is within 0.1 LUFS of target
        assert!(
            (final_loudness - target_loudness).abs() < 0.1,
            "Expected loudness {:.1} LUFS, got {:.1} LUFS",
            target_loudness,
            final_loudness
        );

        Ok(())
    }

    #[test]
    fn test_decode_to_samples() -> Result<()> {
        let normalizer = AudioNormalizer::new(-14.0, -1.0);

        // Use a real WAV file from samples directory
        let wav_path = Path::new("samples/test.wav");
        if !wav_path.exists() {
            return Err(anyhow::anyhow!(
                "Test file not found at {}. Please ensure samples/test.wav exists.",
                wav_path.display()
            ));
        }

        println!("Reading test file from: {}", wav_path.display());

        let (samples, track) = normalizer.process_audio_stream(wav_path)?;
        println!(
            "Track info: channels={}, sample_rate={}",
            track.codec_params.channels.unwrap().count(),
            track.codec_params.sample_rate.unwrap()
        );

        println!("Decoded samples count: {}", samples.len());

        if samples.is_empty() {
            return Err(anyhow::anyhow!(
                "No samples were decoded from the test file"
            ));
        }

        // Check sample ranges
        for (i, &sample) in samples.iter().enumerate() {
            if !(sample >= -1.0 && sample <= 1.0) {
                return Err(anyhow::anyhow!(
                    "Sample {} is outside valid range: {}",
                    i,
                    sample
                ));
            }
        }

        // Basic sanity checks
        assert!(samples.len() > 0, "Should have decoded some samples");
        assert!(
            samples.iter().any(|&x| x != 0.0),
            "Samples should not all be zero"
        );

        Ok(())
    }

    #[test]
    fn test_opus_processing() -> Result<()> {
        let normalizer = AudioNormalizer::new(-14.0, -1.0);
        let opus_path = Path::new("./samples/test.opus");

        // Add file existence check
        if !opus_path.exists() {
            return Ok(()); // Skip test if file doesn't exist
                           // Or alternatively:
                           // return Err(anyhow::anyhow!("Test file not found: {}", opus_path.display()));
        }

        // Test Opus file reading
        let (samples, track) = normalizer.process_audio_stream(&opus_path)?;

        // Add more detailed assertions and debug output
        println!("Decoded {} samples", samples.len());
        println!("Sample rate: {}", track.codec_params.sample_rate.unwrap());
        println!("Channels: {}", track.codec_params.channels.unwrap().count());

        assert!(!samples.is_empty(), "Opus samples should not be empty");
        assert_eq!(
            track.codec_params.sample_rate.unwrap(),
            48000,
            "Opus files should be 48kHz"
        );
        assert!(
            track.codec_params.channels.unwrap().count() <= 2,
            "Should be mono or stereo"
        );

        // Validate sample values
        assert!(
            samples.iter().any(|&x| x != 0.0),
            "All samples are zero - likely decoding issue"
        );

        Ok(())
    }

    #[test]
    fn test_convert_opus_to_mp3() -> Result<()> {
        use std::io::Read;

        let test_opus = Path::new("./samples/test.ogg");

        // Skip test if sample file doesn't exist
        if !test_opus.exists() {
            println!("Skipping test_convert_opus_to_mp3 - test.ogg not found");
            return Ok(());
        }

        // Create a temporary output path
        let temp_dir = tempfile::tempdir()?;
        let output_path = temp_dir.path().join("output-test.mp3");

        // Convert to MP3
        AudioConverter::convert_opus_to_mp3(test_opus, &output_path)?;

        // Verify the output file exists and has content
        assert!(output_path.exists(), "Output MP3 file should exist");

        let mut mp3_file = File::open(&output_path)?;
        let mut mp3_content = Vec::new();
        mp3_file.read_to_end(&mut mp3_content)?;

        // Basic MP3 validation - check for MP3 header magic numbers
        assert!(mp3_content.len() > 4, "MP3 file should have content");
        assert!(
            mp3_content
                .windows(2)
                .any(|window| window == [0xFF, 0xFB] || window == [0xFF, 0xFA]),
            "MP3 file should contain valid MP3 frame headers"
        );

        Ok(())
    }
}
