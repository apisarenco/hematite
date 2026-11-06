#!/usr/bin/env bash
set -euo pipefail

mkdir -p artifacts
docker build -t hematite-builder -f build/Dockerfile .
docker run --rm -v "$(pwd)/artifacts:/artifacts" hematite-builder

echo "==> Build complete: artifacts/hematite"
