use anyhow::Result;
use log::{debug, info, warn};
use reqwest::{
    header::{HeaderMap, HeaderValue, AUTHORIZATION},
    Client as ReqwestClient,
};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use tempfile::tempdir;
use tokio::fs;

use crate::audio::AudioNormalizer;

#[derive(Debug, Deserialize)]
pub struct SoundboardSound {
    pub name: String,
    pub sound_id: String,
    pub volume: f32,
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
                Ok(_) => info!("Successfully processed sound: {}", sound.name),
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
        // Normalize the sound
        let normalized_path = normalizer.normalize_file(input_path)?;

        // Read the normalized file
        let normalized_bytes = fs::read(&normalized_path).await?;

        // Update content type to ogg
        self.create_soundboard_sound(guild_id, sound_name, &normalized_bytes, "audio/ogg")
            .await?;

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
        name: &str,
        file_data: &[u8],
        content_type: &str,
    ) -> Result<()> {
        let url = format!("{}/guilds/{}/soundboard-sounds", self.base_url, guild_id);

        let form = reqwest::multipart::Form::new()
            .text("name", name.to_string())
            .part(
                "sound",
                reqwest::multipart::Part::bytes(file_data.to_vec()).mime_str(content_type)?,
            );

        self.client.post(&url).multipart(form).send().await?;
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
}
