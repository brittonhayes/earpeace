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

use crate::audio_normalizer::AudioNormalizer;

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
        normalizer: &AudioNormalizer,
        guild_id: &str,
    ) -> Result<()> {
        // Get guild sounds
        debug!("Fetching soundboard sounds for guild {}", guild_id);
        let sounds = self.get_guild_sounds(guild_id).await?;

        if sounds.is_empty() {
            info!("No soundboard sounds found in guild");
            return Ok(());
        }

        debug!("Found {} soundboard sounds", sounds.len());

        // Create temporary directory for processing
        let temp_dir = tempdir()?;

        for sound in sounds {
            debug!("Processing sound: {}", sound.name);

            // Download sound
            let temp_path = self
                .download_soundboard_sound(&sound, temp_dir.path())
                .await?;

            // Normalize the sound
            match self
                .normalize_and_upload_sound(normalizer, &temp_path, guild_id, &sound.name)
                .await
            {
                Ok(_) => info!("Successfully processed and uploaded sound: {}", sound.name),
                Err(e) => warn!("Failed to process sound {}: {}", sound.name, e),
            }
        }

        Ok(())
    }

    async fn normalize_and_upload_sound(
        &self,
        normalizer: &AudioNormalizer,
        input_path: &Path,
        guild_id: &str,
        sound_name: &str,
    ) -> Result<()> {
        // First get the existing sound to get all its properties
        let sounds = self.get_guild_sounds(guild_id).await?;
        let existing_sound = sounds.iter().find(|s| s.name == sound_name);

        let normalized_path = normalizer.normalize_file(input_path)?;
        let normalized_bytes = fs::read(&normalized_path).await?;

        // Discord expects MP3 files
        match existing_sound {
            Some(sound) => {
                let original_sound_id = sound.sound_id.clone();

                // Upload the new normalized version
                self.create_soundboard_sound(
                    guild_id,
                    &original_sound_id,
                    &sound.name,
                    &normalized_bytes,
                    "audio/mp3",
                )
                .await?;

                // After successful upload, delete the original
                self.delete_soundboard_sound(guild_id, &original_sound_id)
                    .await?;

                debug!("Replaced sound: {} in guild {}", sound_name, guild_id);
            }
            None => {
                warn!(
                    "Could not find existing sound {}, creating new one",
                    sound_name
                );
                self.create_soundboard_sound(
                    guild_id,
                    sound_name,
                    sound_name,
                    &normalized_bytes,
                    "audio/mp3",
                )
                .await?;
            }
        }

        Ok(())
    }

    async fn get_guild_sounds(&self, guild_id: &str) -> Result<Vec<SoundboardSound>> {
        let url = format!("{}/guilds/{}/soundboard-sounds", self.base_url, guild_id);
        let response: SoundboardResponse = self.client.get(&url).send().await?.json().await?;
        Ok(response.items)
    }

    async fn get_soundboard_sound(&self, sound_id: &str) -> Result<Vec<u8>> {
        let url = format!("https://cdn.discordapp.com/soundboard-sounds/{}", sound_id);
        let bytes = self.client.get(&url).send().await?.bytes().await?.to_vec();

        debug!("Getting sound bytes from {}", url);
        Ok(bytes)
    }

    async fn create_soundboard_sound(
        &self,
        guild_id: &str,
        sound_id: &str,
        name: &str,
        file_data: &[u8],
        content_type: &str,
    ) -> Result<()> {
        let url = format!("{}/guilds/{}/soundboard-sounds", self.base_url, guild_id);

        // Get existing sound to preserve emoji data
        let sounds = self.get_guild_sounds(guild_id).await?;
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
    ) -> Result<PathBuf> {
        debug!("Downloading sound: {}", sound.name);
        let sound_bytes = self.get_soundboard_sound(&sound.sound_id).await?;

        // Change extension to .ogg since Discord serves Ogg files
        let output_path = output_dir.join(format!("{}.ogg", sound.name));
        fs::write(&output_path, sound_bytes).await?;

        info!("Downloaded: {}", output_path.display());
        Ok(output_path)
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
        Ok(())
    }
}
