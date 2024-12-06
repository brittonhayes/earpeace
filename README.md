# Earpeace 🎚️

A simple Discord bot that automatically normalizes the volume of soundboard clips to prevent unexpected volume spikes.

[![Add to Discord](https://img.shields.io/badge/Add%20to%20Discord-5865F2?style=for-the-badge&logo=discord&logoColor=white)](https://discord.com/oauth2/authorize?client_id=1312227542652026880)

## Features

- 🔊 Automatically normalizes audio clips uploaded to a Discord soundboard
- 🎯 Maintains consistent volume levels across all audio clips
- ⚡ Low latency processing
- 🛠️ Easy setup and configuration
- 📊 Configurable target loudness and peak ceiling
- 🔄 Supports both Discord bot and local file processing modes

## Why?

Discord soundboards are fun, but volume inconsistency between clips can be pretty jarring. This bot ensures all clips are uploaded at a comfortable, consistent volume level - no more unexpectedly loud clips!

## Setup

1. Clone this repository
2. Create a `.env` file with your Discord bot token:
```
DISCORD_TOKEN=your_token_here
GUILD_ID=your_guild_id
```
3. Run the bot:
```bash
# Normalize all clips in the ./clips directory
earpeace --input-dir ./clips

# List all the soundboard clips in your discord server
earpeace ls

# Normalize all soundboard clips to the default loudness
earpeace normalize

# Normalize all soundboard clips to the custom loudness
earpeace normalize --target-loudness "-16.0" --peak-ceiling "-3.0"
```

## Usage

```bash
earpeace.exe [OPTIONS]

Options:
  -t, --target-loudness <TARGET_LOUDNESS>
          Target loudness in LUFS (default: -18)
  -p, --peak-ceiling <PEAK_CEILING>
          Target peak output in dB (default: -1)
  -i, --input-dir <INPUT_DIR>
          Directory containing local audio files to normalize
  -d, --discord-token <DISCORD_TOKEN>
          Discord bot token with permissions to read the soundboard
  -g, --guild-id <GUILD_ID>
          Discord guild ID to normalize sounds from
  -l, --log-level <LOG_LEVEL>
          Log level (default: info)
  -h, --help
          Print help
```

### Example Output

```
[2024-11-30T02:31:31Z INFO  earpeace] 🎧 EarPeace starting up...
[2024-11-30T02:31:31Z INFO  earpeace] Target loudness: -18 LUFS
[2024-11-30T02:31:31Z INFO  earpeace] Peak ceiling: -1 dB
[2024-11-30T02:31:31Z INFO  earpeace] Processing file: .\samples\clap.mp3
[2024-11-30T02:31:32Z INFO  earpeace] Processing file: .\samples\explosion.mp3
[2024-11-30T02:31:33Z INFO  earpeace] Processing file: .\samples\song.wav
[2024-11-30T02:31:35Z INFO  earpeace] Normalization complete!
```

## Configuration

The bot uses sensible defaults, but you can adjust the following settings either through command-line options or in your `.env` file:

- Target Loudness: -18 LUFS (adjustable via `--target-loudness`)
- Peak Ceiling: -1 dB (adjustable via `--peak-ceiling`)
- Log Level: info (adjustable via `--log-level`)