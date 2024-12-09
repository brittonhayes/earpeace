use anyhow::Result;
use base64::{engine::general_purpose::STANDARD as base64, Engine};
use log::{debug, info, warn};
use reqwest::{
    header::{HeaderMap, HeaderValue, AUTHORIZATION},
    Client as ReqwestClient,
};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use tempfile::tempdir;
use tokio::fs;

use crate::{
    audio_converter::{AudioConverter, OpusFile},
    audio_file::AudioFile,
    dsp::AudioProcessor,
};
use crate::{audio_file::Mp3File, dsp::decode_file};

#[derive(Debug, Deserialize)]
pub struct SoundboardSound {
    pub name: String,
    pub sound_id: String,
    pub volume: f32,
    pub emoji_id: Option<String>,
    pub emoji_name: Option<String>,
    pub available: Option<bool>,
    pub override_path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SoundboardResponse {
    items: Vec<SoundboardSound>,
}

#[derive(Debug, Deserialize)]
pub struct SoundboardDownload {
    pub bytes: Vec<u8>,
    pub mime_type: String,
}

pub struct DiscordClient {
    client: ReqwestClient,
    base_url: String,
}

impl DiscordClient {
    pub fn new(token: &str) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bot {}", token))?,
        );

        let client = ReqwestClient::builder().default_headers(headers).build()?;

        Ok(Self {
            client,
            base_url: "https://discord.com/api/v10".to_string(),
        })
    }

    pub async fn process_guild_sounds(
        &self,
        processor: &dyn AudioProcessor,
        sounds: Vec<SoundboardSound>,
        guild_id: &str,
    ) -> Result<()> {
        // Create temporary directory for processing
        let temp_dir = tempdir()?;

        for sound in sounds {
            // Download sound
            let (download, temp_path) = self
                .download_soundboard_sound(&sound, temp_dir.path())
                .await?;

            // Define the MP3 output path
            let mp3_path = temp_path.with_extension("mp3");

            // Convert to MP3 if needed
            let processing_path = if download.mime_type == "audio/ogg" {
                let opus_file = OpusFile::new();
                opus_file.convert(&temp_path, &mp3_path)?;
                mp3_path
            } else {
                temp_path
            };

            // Normalize the sound
            match self
                .process_and_upload_sound(processor, &processing_path, guild_id, &sound.name)
                .await
            {
                Ok(_) => info!("Successfully processed and uploaded sound: {}", sound.name),
                Err(e) => warn!("Failed to process sound '{}': {}", sound.name, e),
            }
        }

        Ok(())
    }

    async fn process_and_upload_sound(
        &self,
        processor: &dyn AudioProcessor,
        input_path: &Path,
        guild_id: &str,
        sound_name: &str,
    ) -> Result<()> {
        // Now process the converted file
        let (samples, track) = decode_file(input_path)?;
        let channels = track.codec_params.channels.unwrap().count();
        let sample_rate = track.codec_params.sample_rate.unwrap();

        let normalized_samples = processor.process(&samples, channels, sample_rate)?;

        let mp3 = Mp3File::new();
        let bytes = mp3.write_to_buffer(&normalized_samples, &track)?;

        // Discord expects MP3 files
        let sounds = self.get_guild_sounds(guild_id).await?;
        let existing_sound = sounds.iter().find(|s| s.name == sound_name);

        match existing_sound {
            Some(sound) => {
                let original_sound_id = sound.sound_id.clone();

                // Upload the new normalized version
                self.create_soundboard_sound(
                    &sounds,
                    guild_id,
                    &original_sound_id,
                    &sound.name,
                    &bytes,
                    "audio/mp3",
                )
                .await?;

                // After successful upload, delete the original
                self.delete_soundboard_sound(guild_id, &original_sound_id)
                    .await?;
            }
            None => {
                warn!(
                    "Could not find existing sound {}, creating new one",
                    sound_name
                );
                self.create_soundboard_sound(
                    &sounds,
                    guild_id,
                    sound_name,
                    sound_name,
                    &bytes,
                    "audio/mp3",
                )
                .await?;
            }
        }

        Ok(())
    }

    pub async fn get_guild_sounds(&self, guild_id: &str) -> Result<Vec<SoundboardSound>> {
        let url = format!("{}/guilds/{}/soundboard-sounds", self.base_url, guild_id);
        debug!("Fetching soundboard sounds for guild {}", guild_id);
        let response: SoundboardResponse = self.client.get(&url).send().await?.json().await?;
        debug!("Found {} soundboard sounds", response.items.len());
        Ok(response.items)
    }

    async fn get_soundboard_sound(&self, sound_id: &str) -> Result<SoundboardDownload> {
        let url = format!("https://cdn.discordapp.com/soundboard-sounds/{}", sound_id);
        let response = self.client.get(&url).send().await?;
        let mime_type = response
            .headers()
            .get("Content-Type")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        debug!("Mime type: {}", mime_type);
        let bytes = response.bytes().await?.to_vec();
        Ok(SoundboardDownload { bytes, mime_type })
    }

    async fn create_soundboard_sound(
        &self,
        sounds: &[SoundboardSound],
        guild_id: &str,
        sound_id: &str,
        name: &str,
        file_data: &[u8],
        content_type: &str,
    ) -> Result<()> {
        let url = format!("{}/guilds/{}/soundboard-sounds", self.base_url, guild_id);

        // Get existing sound to preserve emoji data
        let existing_sound = sounds.iter().find(|s| s.sound_id == sound_id);

        // Encode the file data as base64
        let encoded = base64.encode(file_data);
        let sound_data = format!("data:{};base64,{}", content_type, encoded);

        // Create the JSON payload with all original parameters including emojis
        let payload = if let Some(sound) = existing_sound {
            serde_json::json!({
                "name": name,
                "sound_id": sound_id,
                "volume": 1.0,
                "sound": sound_data,
                "emoji_id": sound.emoji_id,
                "emoji_name": sound.emoji_name,
            })
        } else {
            serde_json::json!({
                "name": name,
                "sound_id": sound_id,
                "volume": 1.0,
                "sound": sound_data,
            })
        };

        let response = self.client.post(&url).json(&payload).send().await?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "Failed to create soundboard sound: {}",
                response.text().await?
            ));
        }
        Ok(())
    }

    pub async fn list_guild_sounds(&self, guild_id: &str) -> Result<Vec<SoundboardSound>> {
        let url = format!("{}/guilds/{}/soundboard-sounds", self.base_url, guild_id);
        let response = self.client.get(&url).send().await?;

        let sounds: SoundboardResponse = response.json().await?;
        Ok(sounds.items)
    }

    pub async fn download_soundboard_sound(
        &self,
        sound: &SoundboardSound,
        output_dir: &Path,
    ) -> Result<(SoundboardDownload, PathBuf)> {
        debug!("Downloading sound: {}", sound.name);
        let download = self.get_soundboard_sound(&sound.sound_id).await?;

        debug!(
            "Sound details - Name: {}, ID: {}, Volume: {}, Emoji ID: {:?}, Emoji Name: {:?}, Override Path: {:?}, Size: {} bytes",
            sound.name,
            sound.sound_id,
            sound.volume,
            sound.emoji_id,
            sound.emoji_name,
            sound.override_path,
            download.bytes.len()
        );

        let output_path = output_dir.join(sound.name.as_str());

        fs::write(&output_path, &download.bytes).await?;

        info!("Downloaded: {}", output_path.display());
        Ok((download, output_path))
    }

    // Add new method to delete a sound
    async fn delete_soundboard_sound(&self, guild_id: &str, sound_id: &str) -> Result<()> {
        let url = format!(
            "{}/guilds/{}/soundboard-sounds/{}",
            self.base_url, guild_id, sound_id
        );

        let response = self.client.delete(&url).send().await?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "Failed to delete soundboard sound: {}",
                response.text().await?
            ));
        }

        debug!("Deleted sound: {} in guild {}", sound_id, guild_id);
        Ok(())
    }
}
