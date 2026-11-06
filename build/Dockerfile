FROM rust:1.78-alpine AS builder

RUN apk add --no-cache musl-dev gcc make sqlite-dev sqlite-static

WORKDIR /build

# Cache dependency layer
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && cargo build --release || true

# Copy source and custom plugins
COPY . .

# Trigger re-generation via build.rs
RUN touch build.rs && cargo build --release --target x86_64-unknown-linux-musl

CMD ["cp", "target/x86_64-unknown-linux-musl/release/hematite", "/artifacts/hematite"]
