# Multi-stage build for Helix CLI
FROM rust:slim-bookworm AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    libopenblas-dev \
    g++ \
    make \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /usr/src/helix

# Create dummy files to cache Cargo dependencies
COPY Cargo.toml Cargo.lock ./
COPY ruvector-sona/Cargo.toml ./ruvector-sona/Cargo.toml
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN mkdir -p ruvector-sona/src && touch ruvector-sona/src/lib.rs

# Pre-compile dependencies
RUN cargo build --release

# Copy actual source code
COPY . .

# Re-touch dummy files to ensure they are rebuilt with actual code
RUN touch src/main.rs ruvector-sona/src/lib.rs

# Build the release binary
RUN cargo build --release

# Final runtime stage
FROM debian:bookworm-slim

# Install runtime dependencies (OpenBLAS and CA certificates for TLS requests)
RUN apt-get update && apt-get install -y \
    libopenblas0 \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Copy binary from builder
COPY --from=builder /usr/src/helix/target/release/helix /usr/local/bin/helix

WORKDIR /workspace
ENTRYPOINT ["/usr/local/bin/helix"]
