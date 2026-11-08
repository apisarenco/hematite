# Multi-stage Dockerfile for running Hematite as UID 33 (www-data)
FROM rust:1-bookworm AS builder

WORKDIR /build

# Build argument for version injection
ARG HEMATITE_VERSION
ENV HEMATITE_VERSION=${HEMATITE_VERSION}

# Cache dependencies
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && cargo build --release || true

# Copy source, plugins, templates, and static assets
COPY build.rs ./
COPY src ./src
COPY templates ./templates
COPY static ./static
COPY .git ./.git

# Build release binary
RUN touch build.rs && cargo build --release

# Runtime image
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/hematite /usr/local/bin/hematite

WORKDIR /app

ENV HEMATITE_CONTENT_DIR=/content
ENV HEMATITE_DB_PATH=/data/hematite.db
ENV HEMATITE_BIND_ADDR=0.0.0.0:3000

ENTRYPOINT ["/usr/local/bin/hematite"]
