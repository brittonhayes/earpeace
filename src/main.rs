use anyhow::Result;
use clap::Parser;
use dotenv::dotenv;
use env_logger::{Builder, Target};
use log::{info, LevelFilter};
use std::env;
use std::fs;
use std::path::Path;
use tokio;

mod audio;
mod discord;

#[derive(Parser)]
#[command(
    name = "earpeace",
    about = "A friendly tool to normalize the loudness of Discord soundboard clips ✨🎧"
)]
struct Cli {
    /// Target loudness in LUFS (default: -18)
    #[arg(short, long, default_value = "-18.0")]
    target_loudness: f64,

    /// Target peak output in dB (default: -1)
    #[arg(short, long, default_value = "-1.0")]
    peak_ceiling: f64,

    /// Directory containing local audio files to normalize (optional)
    #[arg(short, long)]
    input_dir: Option<String>,

    /// Discord bot token with permissions to read the soundboard (optional)
    #[arg(short, long)]
    discord_token: Option<String>,

    /// Discord guild ID to normalize sounds from (optional)
    #[arg(short = 'g', long)]
    guild_id: Option<String>,

    /// Log level (default: info)
    #[arg(short, long, default_value = "info")]
    log_level: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Load .env file before parsing CLI args
    dotenv().ok();

    let cli = Cli::parse();
    set_log_level(&cli.log_level);

    info!("🎧 EarPeace starting up...");
    info!("Target loudness: {} LUFS", cli.target_loudness);
    info!("Peak ceiling: {} dB", cli.peak_ceiling);

    let audio = audio::AudioNormalizer::new(cli.target_loudness, cli.peak_ceiling);

    // Modified match statement to check env vars if CLI args aren't provided
    match (&cli.input_dir, &cli.discord_token, &cli.guild_id) {
        (Some(dir), None, None) => {
            process_directory(&audio, dir)?;
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
            discord_client.process_guild_sounds(&audio, &guild).await?;
        }
        _ => {
            info!("Please provide either an input directory (-i) or Discord credentials (via CLI args or .env file)");
            std::process::exit(1);
        }
    }

    info!("Normalization complete!");
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
        .target(Target::Stdout)
        .filter_level(log_level)
        .filter_module(env!("CARGO_PKG_NAME"), log_level)
        .filter_module("symphonia", LevelFilter::Off)
        .filter_module("ebur128", LevelFilter::Off)
        .init();
}
