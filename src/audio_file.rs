use anyhow::{Context, Result};
use log::debug;
use mp3lame_encoder::{Bitrate, Builder, DualPcm, FlushNoGap, Quality};
use std::fs::File;
use std::io::Write;
use std::path::Path;
use symphonia::core::formats::Track;

/// Writes normalized audio samples to an MP3 file using LAME encoding
///
/// # Arguments
/// * `output_path` - Path where the MP3 file will be written
/// * `samples` - Normalized float audio samples in the range [-1.0, 1.0]
/// * `track` - Track metadata containing codec parameters
pub fn write_mp3(samples: &[f32], track: Track, output_path: &Path) -> Result<()> {
    let channels = track.codec_params.channels.unwrap().count();
    let sample_rate = track.codec_params.sample_rate.unwrap();

    let encoder = configure_encoder(channels, sample_rate)?;
    let samples_i16 = convert_samples_to_i16(samples);
    let (left, right) = split_channels(&samples_i16, channels);

    write_encoded_mp3(output_path, encoder, &left, &right)?;

    debug!("Wrote normalized MP3 to: {}", output_path.display());
    Ok(())
}

/// Configures the LAME MP3 encoder with optimal settings
fn configure_encoder(channels: usize, sample_rate: u32) -> Result<mp3lame_encoder::Encoder> {
    let mut builder = Builder::new().context("Failed to create LAME builder")?;
    let _ = builder.set_num_channels(channels as u8);
    let _ = builder.set_sample_rate(sample_rate);
    let _ = builder.set_brate(Bitrate::Kbps192);
    let _ = builder.set_quality(Quality::Best);

    Ok(builder.build().unwrap())
}

/// Converts normalized float samples to 16-bit integer samples
fn convert_samples_to_i16(samples: &[f32]) -> Vec<i16> {
    /// The range of values for 16-bit PCM audio samples
    const I16_RANGE: (f32, f32) = (-32768.0, 32767.0);

    samples
        .iter()
        .map(|&x| (x * I16_RANGE.1).clamp(I16_RANGE.0, I16_RANGE.1) as i16)
        .collect()
}

/// Splits interleaved samples into separate left and right channels
fn split_channels(samples: &[i16], channels: usize) -> (Vec<i16>, Vec<i16>) {
    if channels == 2 {
        let mut left = Vec::with_capacity(samples.len() / 2);
        let mut right = Vec::with_capacity(samples.len() / 2);

        for chunk in samples.chunks(2) {
            left.push(chunk[0]);
            right.push(if chunk.len() > 1 { chunk[1] } else { chunk[0] });
        }
        (left, right)
    } else {
        // Mono: duplicate the same channel
        (samples.to_vec(), samples.to_vec())
    }
}

/// Writes the encoded MP3 data to the output file
fn write_encoded_mp3(
    output_path: &Path,
    mut encoder: mp3lame_encoder::Encoder,
    left: &[i16],
    right: &[i16],
) -> Result<()> {
    let mut output_file = File::create(output_path).context("Failed to create output MP3 file")?;
    let mut mp3_buffer =
        vec![std::mem::MaybeUninit::uninit(); mp3lame_encoder::max_required_buffer_size(1024)];

    // Encode chunks of 1024 samples at a time
    for (left_chunk, right_chunk) in left.chunks(1024).zip(right.chunks(1024)) {
        let input = DualPcm {
            left: left_chunk,
            right: right_chunk,
        };

        let encoded = encoder.encode(input, &mut mp3_buffer).unwrap();
        write_buffer_to_file(&mut output_file, &mp3_buffer, encoded)?;
    }

    // Flush remaining samples
    let final_bytes = encoder.flush::<FlushNoGap>(&mut mp3_buffer).unwrap();
    write_buffer_to_file(&mut output_file, &mp3_buffer, final_bytes)?;

    Ok(())
}

/// Safely writes the encoded buffer to the output file
fn write_buffer_to_file(
    file: &mut File,
    buffer: &[std::mem::MaybeUninit<u8>],
    size: usize,
) -> Result<()> {
    file.write_all(unsafe { std::slice::from_raw_parts(buffer.as_ptr() as *const u8, size) })?;
    Ok(())
}

/// Check if the file extension is ".ogg"
pub fn is_opus_file(path: &Path) -> bool {
    path.extension().map_or(false, |ext| ext == "ogg")
}
