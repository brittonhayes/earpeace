use anyhow::Result;
use poise::serenity_prelude as serenity;
use std::sync::Arc;

use earpeace::audio_normalizer::Normalizer;
use earpeace::discord::DiscordClient;
// Type aliases for convenience
type Error = Box<dyn std::error::Error + Send + Sync>;
type Context<'a> = poise::Context<'a, Data, Error>;

// User data, which is stored and accessible in all command invocations
pub struct Data {
    discord_client: Arc<DiscordClient>,
}

async fn on_error(error: poise::FrameworkError<'_, Data, Error>) {
    // This is our custom error handler
    // They are many errors that can occur, so we only handle the ones we want to customize
    // and forward the rest to the default handler
    match error {
        poise::FrameworkError::Setup { error, .. } => panic!("Failed to start bot: {:?}", error),
        poise::FrameworkError::Command { error, ctx, .. } => {
            println!("Error in command `{}`: {:?}", ctx.command().name, error,);
        }
        error => {
            if let Err(e) = poise::builtins::on_error(error).await {
                println!("Error while handling error: {}", e)
            }
        }
    }
}

/// Normalize all soundboard sounds in the current guild
#[poise::command(slash_command, guild_only)]
async fn normalize(
    ctx: Context<'_>,
    #[description = "Target loudness in LUFS (default: -18.0)"] target_loudness: Option<f64>,
) -> Result<(), Error> {
    // Defer the response since this might take a while
    ctx.defer().await?;

    let guild_id = ctx.guild_id().unwrap().to_string();

    let target_loudness = target_loudness.unwrap_or(Normalizer::DEFAULT_TARGET_LOUDNESS);
    let target_peak = Normalizer::DEFAULT_TARGET_PEAK;

    let audio_normalizer = match Normalizer::new(target_loudness, target_peak) {
        Ok(normalizer) => normalizer,
        Err(e) => {
            let error_message = format!("❌ Invalid options: {}", e);
            ctx.say(error_message).await?;
            return Ok(());
        }
    };

    ctx.say("Starting sound normalization process...").await?;

    let sounds = ctx
        .data()
        .discord_client
        .get_guild_sounds(&guild_id)
        .await?;

    // Process all guild sounds
    match ctx
        .data()
        .discord_client
        .process_guild_sounds(&audio_normalizer, sounds, &guild_id)
        .await
    {
        Ok(_) => {
            ctx.say("✅ Successfully normalized all soundboard sounds!")
                .await?;
        }
        Err(e) => {
            ctx.say(format!("❌ Error normalizing sounds: {}", e))
                .await?;
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() {
    // Initialize logging at debug level
    env_logger::builder()
        .filter_module(module_path!(), log::LevelFilter::Off)
        .filter_module("earpeace", log::LevelFilter::Debug)
        .init();

    // Load environment variables from .env file
    dotenv::dotenv().ok();

    // Get Discord token from environment
    let token = std::env::var("DISCORD_TOKEN").expect("missing DISCORD_TOKEN");
    let intents = serenity::GatewayIntents::non_privileged();

    // Initialize shared components
    let discord_client =
        Arc::new(DiscordClient::new(&token).expect("Failed to create Discord client"));

    let options = poise::FrameworkOptions {
        commands: vec![normalize()],
        on_error: |error| Box::pin(on_error(error)),
        ..Default::default()
    };

    let framework = poise::Framework::builder()
        .setup(|ctx, _ready, framework| {
            Box::pin(async move {
                println!(
                    "Logged in as {} with session {}",
                    _ready.user.name, _ready.session_id
                );
                poise::builtins::register_globally(ctx, &framework.options().commands).await?;
                Ok(Data { discord_client })
            })
        })
        .options(options)
        .build();

    let client = serenity::ClientBuilder::new(token, intents)
        .framework(framework)
        .await;

    client.unwrap().start().await.unwrap();
}
