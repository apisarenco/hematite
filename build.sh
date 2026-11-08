#!/usr/bin/env bash
set -euo pipefail

VERSION="${HEMATITE_VERSION:-$(git describe --tags --match "v[0-9]*" --abbrev=0 2>/dev/null || echo "v0.0.0")}"
echo "==> Building Hematite version: ${VERSION}"

mkdir -p artifacts
docker build --build-arg HEMATITE_VERSION="${VERSION}" -t hematite-builder -f build/Dockerfile .
docker run --rm -v "$(pwd)/artifacts:/artifacts" hematite-builder

echo "==> Build complete: artifacts/hematite"
