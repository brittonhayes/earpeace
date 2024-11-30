use anyhow::Result;
use log::{info, warn};
use reqwest::{
    header::{HeaderMap, HeaderValue, AUTHORIZATION},
    Client as ReqwestClient,
};
use serde::Deserialize;
use std::path::PathBuf;
use tempfile::tempdir;
use tokio::fs;

use crate::audio::AudioNormalizer;

#[derive(Debug, Deserialize)]
struct SoundboardSound {
    id: String,
    name: String,
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
        info!("Fetching soundboard sounds for guild {}", guild_id);
        let sounds = self.get_guild_sounds(guild_id).await?;

        if sounds.is_empty() {
            info!("No soundboard sounds found in guild");
            return Ok(());
        }

        info!("Found {} soundboard sounds", sounds.len());

        // Create temporary directory for processing
        let temp_dir = tempdir()?;

        for sound in sounds {
            info!("Processing sound: {}", sound.name);

            // Download sound
            let sound_bytes = self.get_soundboard_sound(guild_id, &sound.id).await?;

            // Save to temp file
            let temp_path = temp_dir.path().join(format!("{}.mp3", sound.name));
            fs::write(&temp_path, sound_bytes).await?;

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
        input_path: &PathBuf,
        guild_id: &str,
        sound_name: &str,
    ) -> Result<()> {
        // Normalize the sound
        normalizer.normalize_file(input_path)?;

        // Get the normalized file path
        let normalized_path = input_path.with_file_name(format!(
            "{}-normalized.wav",
            input_path.file_stem().unwrap().to_string_lossy()
        ));

        // Read the normalized file
        let normalized_bytes = fs::read(&normalized_path).await?;

        // Upload the normalized sound
        self.create_soundboard_sound(guild_id, sound_name, &normalized_bytes, "audio/wav")
            .await?;

        Ok(())
    }

    async fn get_guild_sounds(&self, guild_id: &str) -> Result<Vec<SoundboardSound>> {
        let url = format!("{}/guilds/{}/soundboard-sounds", self.base_url, guild_id);
        let sounds: Vec<SoundboardSound> = self.client.get(&url).send().await?.json().await?;
        Ok(sounds)
    }

    async fn get_soundboard_sound(&self, guild_id: &str, sound_id: &str) -> Result<Vec<u8>> {
        let url = format!(
            "{}/guilds/{}/soundboard-sounds/{}",
            self.base_url, guild_id, sound_id
        );
        let bytes = self.client.get(&url).send().await?.bytes().await?.to_vec();
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
}
