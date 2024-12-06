use anyhow::{Context, Result};
use log::debug;
use std::path::Path;
use std::process::Command;

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
