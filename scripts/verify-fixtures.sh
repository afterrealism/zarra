#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
for dir in crates/zarra/tests/data crates/zarra-cli/tests/data; do
  diff -r testdata/refpy.zarr "$dir/refpy.zarr"
done
cmp testdata/sample.tif crates/zarra-cli/tests/data/sample.tif
printf '%s  %s\n' \
  a609e165541e8c062b7662564e6d64c4e5c6e94ecba914dc9202d041a2f66104 testdata/refpy.zarr/0/zarr.json \
  8e390da5ecd278c318d586f112d1484e7836047e22b0d487fbef32780b071e17 testdata/refpy.zarr/0/c/0/0/0 \
  9b4604471f6a2bb5674ba990c28c3f51c26d1c201d849581225c968d25dd2664 testdata/sample.tif \
  | sha256sum -c -
