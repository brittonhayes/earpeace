# Build stage
FROM rust:1.76-slim-bullseye as builder

# Install required dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    build-essential \
    ffmpeg \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy the entire project
COPY . .

# Build the release binary
RUN cargo build --release --bin bot

# Runtime stage
FROM debian:bullseye-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl1.1 \
    ffmpeg \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy the binary from builder
COPY --from=builder /app/target/release/bot /app/bot

# Set the startup command
CMD ["./bot"] 