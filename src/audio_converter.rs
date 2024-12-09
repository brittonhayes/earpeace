use log::debug;
use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;

/// Errors that can occur during audio conversion
#[derive(Error, Debug)]
pub enum ConversionError {
    #[error("FFmpeg command failed to execute: {0}")]
    FFmpegExecutionError(#[from] std::io::Error),
    #[error("FFmpeg conversion failed with non-zero exit status")]
    FFmpegConversionError,
}

pub trait AudioConverter {
    fn convert(&self, input_path: &Path, output_path: &Path) -> Result<PathBuf, ConversionError>;
}

pub struct OpusFile;

impl Default for OpusFile {
    fn default() -> Self {
        Self::new()
    }
}

impl OpusFile {
    pub fn new() -> Self {
        Self
    }
}

impl AudioConverter for OpusFile {
    fn convert(&self, input_path: &Path, output_path: &Path) -> Result<PathBuf, ConversionError> {
        // TODO: This is a temporary solution. We should use a Rust library to convert the audio file.
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
                "quiet",
            ])
            .status()?;

        if !status.success() {
            return Err(ConversionError::FFmpegConversionError);
        }

        Ok(output_path.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs::File, io::Read, path::PathBuf};

    #[test]
    fn test_convert_opus_to_mp3_invalid_input() {
        let input = PathBuf::from("nonexistent.opus");
        let output = PathBuf::from("output.mp3");

        let result = OpusFile.convert(&input, &output);
        assert!(result.is_err());
    }

    #[test]
    fn test_convert_opus_to_mp3() {
        let test_opus = Path::new("./samples/test.ogg");

        // Create a temporary output path
        let temp_dir = tempfile::tempdir().unwrap();
        let output_path = temp_dir.path().join("output-test.mp3");

        // Convert to MP3
        OpusFile.convert(test_opus, &output_path).unwrap();

        // Verify the output file exists and has content
        assert!(output_path.exists(), "Output MP3 file should exist");

        let mut mp3_file = File::open(&output_path).unwrap();
        let mut mp3_content = Vec::new();
        mp3_file.read_to_end(&mut mp3_content).unwrap();

        // Basic MP3 validation - check for MP3 header magic numbers
        assert!(mp3_content.len() > 4, "MP3 file should have content");
        assert!(
            mp3_content
                .windows(2)
                .any(|window| window == [0xFF, 0xFB] || window == [0xFF, 0xFA]),
            "MP3 file should contain valid MP3 frame headers"
        );
    }
}
