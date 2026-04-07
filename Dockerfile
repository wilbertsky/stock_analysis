# ── Build stage ───────────────────────────────────────────────────────────────
FROM rust:1.88-slim AS builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y pkg-config libssl-dev curl && rm -rf /var/lib/apt/lists/*

# Cache dependencies by copying manifests first
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release
RUN rm src/main.rs

# Build the real binary
COPY src ./src
COPY migrations ./migrations
RUN touch src/main.rs && cargo build --release

# ── Runtime stage ─────────────────────────────────────────────────────────────
FROM debian:bookworm-slim

WORKDIR /app

RUN apt-get update && apt-get install -y ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/axum-api ./axum-api
COPY --from=builder /app/migrations ./migrations

EXPOSE 8080

ENV RUST_LOG=info

CMD ["./axum-api"]
