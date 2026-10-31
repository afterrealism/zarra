# zarra-cli

`zarra-cli` installs the `zarra` command-line binary for canonical multiscale Zarr v3 raster stores. It uses the `zarra` library for all conversion, query, and rendering behavior; there is no second raster decoder in the CLI or viewer.

## Path installation

Version 0.1.0 is Unreleased. Install from this checkout while crates.io publication is deferred by the upstream `zarrs 0.23.15` release gate:

```sh
cargo install --path crates/zarra-cli
zarra --help
```

The intended registry destination after publication is [crates.io/crates/zarra-cli](https://crates.io/crates/zarra-cli); this is not a publication claim.

## Commands

```sh
zarra convert input.tif -o output.zarr --maxzoom 12 --resampling bilinear --zstd-level 5
zarra info output.zarr
zarra query output.zarr --level 0 --x 0 --y 0 --width 8 --height 8
zarra sql output.zarr "SELECT band, AVG(value) FROM raster GROUP BY band"
zarra tile output.zarr --z 0 --x 0 --y 0 -o tile.png
zarra serve output.zarr --port 8090 --upload-root uploads
```

`info`, `query`, `sql`, and `tile` also accept `--url <URL>` instead of a positional local store. HTTPS and loopback HTTP are supported for trusted read-only stores; upstream HTTP behavior has no total-timeout, request/byte-budget, full-body avoidance, or HTTPS-downgrade guarantee.

Conversion requires GDAL 3.13+ (`GDAL_BIN_DIR` optionally selects its executable directory). Existing store and tile outputs are never overwritten. Empty tiles create no file. PNG output is quantized for display rather than lossless scientific data. Conversion defaults to native-zoom detection, automatic categorical/bilinear resampling, and ZSTD level 5; `--maxzoom`, `--resampling`, and `--zstd-level` adjust those choices.

`sql` stages the selected level/window in fresh in-memory SQLite as `raster(x, y, band, value)`, with NaN and declared nodata represented as `NULL`. It accepts one read-only statement, stages at most 1,048,576 band values, interrupts execution after 10 seconds, and emits bounded compact JSON (up to 1,000 rows / 8 MiB, including metadata and escaping).

`serve` owns the loopback nested-object range server, read-only SQL endpoint, optional no-overwrite GeoTIFF upload endpoint, and embedded MapLibre 4.7.1 viewer. `--upload-root` enables conversion uploads and is disabled by default. The viewer can adjust conversion settings, open the converted active store, and query its actual pixels. It serves real Rust-rendered tiles and all vendored assets, including the MapLibre/Mapbox/glfx/d3 license notices. The `example/` directory only launches and tests this same binary.

See the [workspace README](https://github.com/afterrealism/zarra#readme) for query layout, band/alpha semantics, the exact support matrix, HTTP limitations, and all verification commands.

## Verification and license

Run `cargo test -p zarra-cli --all-features --locked` from the workspace. `bash scripts/test-gdal.sh` additionally runs the real GDAL 3.13.1 conversion, custom-options, and HTTP upload integration suites in Docker, and `npm --prefix example test` builds this binary for the Node 20+ server/viewer/SQL integration tests.

Licensed under Apache-2.0 OR MIT. See the workspace `LICENSE-APACHE`, `LICENSE-MIT`, and `ATTRIBUTION.md` files.
