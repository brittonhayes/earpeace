use std::{fs::File, path::Path};

use anyhow::Error;
use symphonia::{
    core::{
        formats::{FormatOptions, Track},
        io::MediaSourceStream,
        meta::MetadataOptions,
        probe::Hint,
    },
    default,
};

pub trait AudioProcessor: Send + Sync {
    fn process(
        &self,
        samples: &[f32],
        channels: usize,
        sample_rate: u32,
    ) -> Result<Vec<f32>, Error>;
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
pub fn decode_to_samples(
    format: &mut Box<dyn symphonia::core::formats::FormatReader>,
    track_id: u32,
    mut decoder: Box<dyn symphonia::core::codecs::Decoder>,
) -> Result<Vec<f32>, Error> {
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
pub fn decode_file(input_path: &Path) -> Result<(Vec<f32>, Track), anyhow::Error> {
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
        .ok_or(anyhow::anyhow!("No default track found"))?
        .clone();

    // Get decoder
    let decoder =
        symphonia::default::get_codecs().make(&track.codec_params, &Default::default())?;

    // Decode samples
    let samples = decode_to_samples(&mut format_reader, track.id, decoder)?;

    Ok((samples, track))
}
