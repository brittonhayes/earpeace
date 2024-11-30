use anyhow::{Context, Result};
use ebur128::{EbuR128, Mode};
use lame::Lame;
use log::debug;
use mp3lame_encoder::{Bitrate, Builder, DualPcm, FlushNoGap, Quality};
use std::fs::File;
use std::io::Write;
use std::path::Path;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::formats::Track;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

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

    pub fn normalize_file(&self, input_path: &Path) -> Result<()> {
        // Read and decode the audio file
        let (samples, track) = self.read_audio_file(input_path)?;

        // Calculate and apply normalization
        let normalized_samples = self.normalize_samples(&samples, &track)?;

        // Write the normalized audio
        self.write_normalized_audio(input_path, &normalized_samples, track)?;

        Ok(())
    }

    fn read_audio_file(&self, input_path: &Path) -> Result<(Vec<f32>, Track)> {
        let (mut format, track) = self.probe_audio_format(input_path)?;
        let track_id = track.id;

        let decoder = self.create_decoder(&track)?;
        let samples = self.decode_to_samples(&mut format, track_id, decoder)?;

        Ok((samples, track))
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

    fn probe_audio_format(
        &self,
        input_path: &Path,
    ) -> Result<(Box<dyn symphonia::core::formats::FormatReader>, Track)> {
        let file = File::open(input_path).context("Failed to open audio file")?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());

        let mut hint = Hint::new();
        if let Some(extension) = input_path.extension() {
            hint.with_extension(extension.to_str().unwrap_or(""));
        }

        let format_opts: FormatOptions = Default::default();
        let metadata_opts: MetadataOptions = Default::default();

        let probed = symphonia::default::get_probe()
            .format(&hint, mss, &format_opts, &metadata_opts)
            .context("Failed to probe audio format")?;

        let format = probed.format;
        let track = format
            .default_track()
            .context("No default track found")?
            .clone();

        Ok((format, track))
    }

    fn create_decoder(&self, track: &Track) -> Result<Box<dyn symphonia::core::codecs::Decoder>> {
        symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .context("Failed to create decoder")
    }

    fn normalize_samples(&self, samples: &[f32], track: &Track) -> Result<Vec<f32>> {
        let channels = track.codec_params.channels.unwrap().count();
        let sample_rate = track.codec_params.sample_rate.unwrap();

        let samples_i16: Vec<i16> = samples.iter().map(|&x| (x * 32767.0) as i16).collect();

        // Calculate normalization gain
        let gain = self.calculate_normalization_gain(channels, sample_rate, &samples_i16)?;

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

    fn calculate_normalization_gain(
        &self,
        channels: usize,
        sample_rate: u32,
        samples: &[i16],
    ) -> Result<f64> {
        let mut ebu = EbuR128::new(channels as u32, sample_rate, Mode::I | Mode::TRUE_PEAK)?;

        ebu.add_frames_i16(samples)?;

        let current_loudness = ebu.loudness_global()?;
        let gain_adjustment = self.target_loudness - current_loudness;
        let linear_gain = 10f64.powf(gain_adjustment / 20.0);

        debug!(
            "Current loudness: {:.1} LUFS, applying {:.1} dB gain adjustment",
            current_loudness, gain_adjustment
        );

        Ok(linear_gain)
    }

    fn write_normalized_audio(
        &self,
        input_path: &Path,
        normalized_samples: &[f32],
        track: Track,
    ) -> Result<()> {
        let extension = input_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("wav");

        let output_path = input_path.with_file_name(format!(
            "{}-normalized.{}",
            input_path.file_stem().unwrap().to_string_lossy(),
            extension
        ));

        match extension.to_lowercase().as_str() {
            "wav" => self.write_wav(
                &output_path,
                normalized_samples,
                track.codec_params.channels.unwrap().count(),
                track.codec_params.sample_rate.unwrap(),
            ),
            "mp3" => self.write_mp3(&output_path, normalized_samples, track),
            _ => {
                // For non-WAV/MP3 formats, we'll use the WAV format as a fallback
                debug!(
                    "Format {} not directly supported, falling back to WAV",
                    extension
                );
                self.write_wav(
                    &output_path.with_extension("wav"),
                    normalized_samples,
                    track.codec_params.channels.unwrap().count(),
                    track.codec_params.sample_rate.unwrap(),
                )
            }
        }
    }

    fn write_wav(
        &self,
        output_path: &Path,
        normalized_samples: &[f32],
        channels: usize,
        sample_rate: u32,
    ) -> Result<()> {
        let spec = hound::WavSpec {
            channels: channels as u16,
            sample_rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };

        let mut writer =
            hound::WavWriter::create(output_path, spec).context("Failed to create output file")?;

        for &sample in normalized_samples {
            writer
                .write_sample(sample)
                .context("Failed to write sample")?;
        }

        writer.finalize().context("Failed to finalize WAV file")?;
        debug!("Wrote normalized audio to: {}", output_path.display());

        Ok(())
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
        encoder.set_num_channels(channels as u8);
        encoder.set_sample_rate(sample_rate);
        encoder.set_brate(Bitrate::Kbps192);
        encoder.set_quality(Quality::Good);

        let mut encoder = encoder.build().unwrap();

        // Convert f32 samples to i16 and split into channels
        let samples_i16: Vec<i16> = normalized_samples
            .iter()
            .map(|&x| (x * 32767.0).clamp(-32768.0, 32767.0) as i16)
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
        let mut mp3_buffer = vec![std::mem::MaybeUninit::uninit(); mp3lame_encoder::max_required_buffer_size(1024)];

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_calculate_normalization_gain() {
        let normalizer = AudioNormalizer::new(-14.0, -1.0);
        let test_samples: Vec<i16> = vec![0, 16384, -16384, 32767, -32767];
        let result = normalizer.calculate_normalization_gain(1, 44100, &test_samples);
        assert!(result.is_ok());
    }

    #[test]
    fn test_read_audio_file() -> Result<()> {
        let normalizer = AudioNormalizer::new(-14.0, -1.0);
        let temp_dir = tempdir()?;

        // Create a test WAV file
        let wav_path = temp_dir.path().join("test.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 44100,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        {
            let mut writer = hound::WavWriter::create(&wav_path, spec)?;
            // Write a simple sine wave
            for t in (0..44100).map(|x| x as f32 / 44100.0) {
                let sample = (t * 440.0 * 2.0 * std::f32::consts::PI).sin();
                writer.write_sample(sample)?;
            }
            writer.finalize()?;
        }

        // Test WAV file reading
        let (samples, track) = normalizer.read_audio_file(&wav_path)?;

        assert!(!samples.is_empty(), "WAV samples should not be empty");
        assert_eq!(track.codec_params.sample_rate.unwrap(), 44100);
        assert_eq!(track.codec_params.channels.unwrap().count(), 1);

        Ok(())
    }

    #[test]
    fn test_normalization_gain_achieves_target() -> Result<()> {
        let target_loudness = -14.0;
        let normalizer = AudioNormalizer::new(target_loudness, -1.0);
        let temp_dir = tempdir()?;

        // Create a test WAV file with known amplitude
        let wav_path = temp_dir.path().join("test.wav");
        let channels = 1;
        let sample_rate = 44100;
        let spec = hound::WavSpec {
            channels: channels as u16,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };

        // Generate a 1-second sine wave at high amplitude
        let samples_i16: Vec<i16> = (0..sample_rate)
            .map(|x| {
                let t = x as f32 / sample_rate as f32;
                let sample = (t * 440.0 * 2.0 * std::f32::consts::PI).sin();
                // Use 80% of maximum amplitude
                (sample * 0.8 * i16::MAX as f32) as i16
            })
            .collect();

        {
            let mut writer = hound::WavWriter::create(&wav_path, spec)?;
            for &sample in &samples_i16 {
                writer.write_sample(sample)?;
            }
            writer.finalize()?;
        }

        // Calculate the gain
        let gain = normalizer.calculate_normalization_gain(channels, sample_rate, &samples_i16)?;

        // Apply the gain to get normalized samples
        let normalized_samples: Vec<i16> = samples_i16
            .iter()
            .map(|&s| (s as f64 * gain).clamp(-32768.0, 32767.0) as i16)
            .collect();

        // Measure the loudness of normalized samples
        let mut ebu = EbuR128::new(channels as u32, sample_rate, Mode::I | Mode::TRUE_PEAK)?;
        ebu.add_frames_i16(&normalized_samples)?;
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

        let (mut format, track) = normalizer.probe_audio_format(&wav_path)?;
        println!(
            "Track info: channels={}, sample_rate={}",
            track.codec_params.channels.unwrap().count(),
            track.codec_params.sample_rate.unwrap()
        );

        let decoder = normalizer.create_decoder(&track)?;
        let decoded_samples = normalizer.decode_to_samples(&mut format, track.id, decoder)?;

        println!("Decoded samples count: {}", decoded_samples.len());

        if decoded_samples.is_empty() {
            return Err(anyhow::anyhow!(
                "No samples were decoded from the test file"
            ));
        }

        // Check sample ranges
        for (i, &sample) in decoded_samples.iter().enumerate() {
            if !(sample >= -1.0 && sample <= 1.0) {
                return Err(anyhow::anyhow!(
                    "Sample {} is outside valid range: {}",
                    i,
                    sample
                ));
            }
        }

        // Basic sanity checks
        assert!(
            decoded_samples.len() > 0,
            "Should have decoded some samples"
        );
        assert!(
            decoded_samples.iter().any(|&x| x != 0.0),
            "Samples should not all be zero"
        );

        Ok(())
    }
}
