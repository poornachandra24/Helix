# Multi-stage build for Helix CLI
FROM rust:1.85-slim-bookworm AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    libopenblas-dev \
    g++ \
    make \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /usr/src/helix
COPY . .

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
