use anyhow::Result;
use clap::{Parser, Subcommand};
use dotenv::dotenv;
use env_logger::{Builder, Target};
use log::{info, LevelFilter};
use std::env;
use std::fs;
use std::path::Path;

use earpeace::audio;
use earpeace::discord;

#[derive(Parser)]
#[command(
    name = "earpeace",
    about = "A friendly tool to normalize the loudness of Discord soundboard clips ✨🎧"
)]
struct Cli {
    /// Discord bot token with permissions to read the soundboard (optional)
    #[arg(short, long, global = true)]
    discord_token: Option<String>,

    /// Discord guild ID to normalize sounds from (optional)
    #[arg(short = 'g', long, global = true)]
    guild_id: Option<String>,

    /// Log level (default: info)
    #[arg(short = 'l', long, default_value = "info", global = true)]
    log_level: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Normalize audio files
    Normalize {
        /// Directory containing local audio files to normalize
        #[arg(short, long)]
        input_dir: Option<String>,

        /// Target loudness in LUFS (default: -18)
        #[arg(short = 't', long = "target-loudness", default_value = "-18.0", allow_negative_numbers = true)]
        target_loudness: f64,

        /// Target peak output in dB (default: -1)
        #[arg(short = 'p', long = "peak-ceiling", default_value = "-1.0", allow_negative_numbers = true)]
        peak_ceiling: f64,
    },
    /// List all sounds in the Discord soundboard
    Ls,
    /// Copy sounds from the Discord soundboard to the local directory
    Cp {
        /// Output directory for downloaded sounds
        #[arg(short, long, default_value = ".")]
        output_dir: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Load .env file before parsing CLI args
    dotenv().ok();

    let cli = Cli::parse();
    set_log_level(&cli.log_level);

    match &cli.command {
        Commands::Normalize {
            input_dir,
            target_loudness,
            peak_ceiling,
        } => match (input_dir, &cli.discord_token, &cli.guild_id) {
            (Some(dir), None, None) => {
                let audio = audio::AudioNormalizer::new(*target_loudness, *peak_ceiling);
                process_directory(&audio, dir)?;
            }
            (None, Some(token), Some(guild)) => {
                let audio = audio::AudioNormalizer::new(*target_loudness, *peak_ceiling);
                let discord_client = discord::DiscordClient::new(token)?;
                discord_client.process_guild_sounds(&audio, guild).await?;
            }
            (None, token_opt, guild_opt) => {
                let token = token_opt
                    .clone()
                    .or_else(|| env::var("TOKEN").ok())
                    .ok_or_else(|| anyhow::anyhow!("Discord token not provided in CLI or .env"))?;

                let guild = guild_opt
                    .clone()
                    .or_else(|| env::var("GUILD_ID").ok())
                    .ok_or_else(|| anyhow::anyhow!("Guild ID not provided in CLI or .env"))?;

                let discord_client = discord::DiscordClient::new(&token)?;

                let audio = audio::AudioNormalizer::new(*target_loudness, *peak_ceiling);
                discord_client.process_guild_sounds(&audio, &guild).await?;
            }
            _ => {
                info!("Please provide either an input directory (-i) or Discord credentials");
                std::process::exit(1);
            }
        },
        Commands::Ls => {
            let token = cli
                .discord_token
                .or_else(|| env::var("TOKEN").ok())
                .ok_or_else(|| anyhow::anyhow!("Discord token not provided in CLI or .env"))?;

            let guild = cli
                .guild_id
                .or_else(|| env::var("GUILD_ID").ok())
                .ok_or_else(|| anyhow::anyhow!("Guild ID not provided in CLI or .env"))?;

            let discord_client = discord::DiscordClient::new(&token)?;
            let sounds = discord_client.list_guild_sounds(&guild).await?;

            println!("\n🎵 Discord Soundboard Sounds 🎵\n");

            if sounds.is_empty() {
                println!("No sounds found in guild.");
                return Ok(());
            }

            // Find the longest name for padding
            let max_name_len = sounds.iter().map(|s| s.name.len()).max().unwrap();

            for sound in sounds {
                let volume_bar = "▮".repeat((sound.volume * 10.0) as usize);
                let volume_empty = "▯".repeat(10 - (sound.volume * 10.0) as usize);

                println!(
                    "{:<width$} │ Vol: [{}{}] {:.1}",
                    sound.name,
                    volume_bar,
                    volume_empty,
                    sound.volume,
                    width = max_name_len
                );
            }
            println!();

            return Ok(());
        }

        Commands::Cp { output_dir } => {
            let token = cli
                .discord_token
                .or_else(|| env::var("TOKEN").ok())
                .ok_or_else(|| anyhow::anyhow!("Discord token not provided in CLI or .env"))?;
            let guild = cli
                .guild_id
                .or_else(|| env::var("GUILD_ID").ok())
                .ok_or_else(|| anyhow::anyhow!("Guild ID not provided in CLI or .env"))?;

            let discord_client = discord::DiscordClient::new(&token)?;
            let sounds = discord_client.list_guild_sounds(&guild).await?;
            for sound in sounds {
                discord_client
                    .download_soundboard_sound(&sound, Path::new(&output_dir))
                    .await?;
            }
        }
    }

    Ok(())
}

fn process_directory(normalizer: &audio::AudioNormalizer, dir: &str) -> Result<()> {
    let dir_path = Path::new(dir);
    if !dir_path.is_dir() {
        return Err(anyhow::anyhow!("Provided path is not a directory"));
    }

    for entry in fs::read_dir(dir_path)? {
        let entry = entry?;
        let path = entry.path();

        if let Some(extension) = path.extension() {
            if matches!(extension.to_str(), Some("mp3" | "wav" | "ogg")) {
                info!("Processing file: {}", path.display());
                normalizer.normalize_file(&path)?;
            }
        }
    }

    Ok(())
}

fn set_log_level(level_str: &str) {
    let log_level = match level_str.to_lowercase().as_str() {
        "error" => LevelFilter::Error,
        "warn" => LevelFilter::Warn,
        "info" => LevelFilter::Info,
        "debug" => LevelFilter::Debug,
        "trace" => LevelFilter::Trace,
        _ => LevelFilter::Info, // Default to Info if invalid
    };

    Builder::new()
        .target(Target::Stderr)
        .filter_level(log_level)
        .init();
}
