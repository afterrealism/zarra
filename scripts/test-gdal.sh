#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
image=zarra-gdal-test:3.13.1
docker build -f "$root/scripts/Dockerfile.gdal" -t "$image" "$root/scripts"
docker run --rm \
  --user "$(id -u):$(id -g)" \
  -e HOME=/tmp -e CARGO_HOME=/tmp/.cargo \
  -v "$root:/src" -w /src "$image"
