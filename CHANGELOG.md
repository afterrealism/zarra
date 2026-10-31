# Changelog

All notable changes to Zarra are documented here. The format follows [Keep a Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/), and package versions follow [Semantic Versioning 2.0.0](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - Unreleased

### Added

- Canonical WebMercatorQuad-aligned multiscale Zarr v3 conversion from local GeoTIFF/COG through GDAL 3.13+ CLI, with optional maximum zoom, resampling, and ZSTD level controls, sibling temporary builds, validation, one-rename local publication, and no output overwrite.
- Strict root manifests and sharded level validation for 2D/3D arrays, eight profile dtypes, little-endian bytes, optional inner ZSTD, and CRC32C shard indexes.
- Local and optional keyed-HTTP stores with exact chunk, window, neighborhood, and ordered/repeated-band reads, sparse/NaN semantics, and checked allocation limits.
- Native and deep-overzoom XYZ rendering through zoom 22, RG16/RGB8 quantization, first-selected-band nodata alpha, and no-clobber PNG output.
- The `zarra` binary with `convert`, `info`, `query`, `sql`, `tile`, and local-only `serve` commands.
- Read-only in-memory SQLite analytics over bounded raster windows, with NaN/declared-nodata-as-NULL staging, a 10-second query interrupt, and bounded JSON results.
- A sequential loopback server with real tile/manifest routes, nested-object single-range responses, GET/HEAD/POST/OPTIONS handling, path-escape and cross-site-POST defenses, no-overwrite GeoTIFF uploads, and embedded static assets.
- A MapLibre 4.7.1 raster viewer using the real Rust tile endpoint, declared extent/tile metadata, navigation, fit, pointer telemetry, adjustable GeoTIFF conversion, and SQL result tables.
- Canonical fixtures with per-crate byte-identical copies, stable/MSRV/GDAL/Node integration checks, package documentation, dual licensing, and read-only CI.

### Release status

- Local and path use is implemented. crates.io publication remains deferred until `cargo info zarrs@0.23.15` succeeds. The pinned full-SHA `zarrs` backport is retained for local/path builds; package-list checks are file-inclusion checks only, not publish readiness.
