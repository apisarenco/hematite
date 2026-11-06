# Multi-stage Dockerfile for running Hematite as UID 33 (www-data)
FROM rust:1-bookworm AS builder

WORKDIR /build

# Cache dependencies
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && cargo build --release || true

# Copy source, plugins, templates, and static assets
COPY build.rs ./
COPY src ./src
COPY templates ./templates
COPY static ./static

# Build release binary
RUN touch build.rs && cargo build --release

# Runtime image
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Create app directory and data directory writable by UID 33 (www-data)
RUN mkdir -p /app /data /content && chown -R 33:33 /app /data

COPY --from=builder /build/target/release/hematite /usr/local/bin/hematite

WORKDIR /app
USER 33:33

ENV HEMATITE_CONTENT_DIR=/content
ENV HEMATITE_DB_PATH=/data/hematite.db
ENV HEMATITE_BIND_ADDR=0.0.0.0:3000

ENTRYPOINT ["/usr/local/bin/hematite"]
