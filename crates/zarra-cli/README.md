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
zarra convert input.tif -o output.zarr
zarra info output.zarr
zarra query output.zarr --level 0 --x 0 --y 0 --width 8 --height 8
zarra tile output.zarr --z 0 --x 0 --y 0 -o tile.png
zarra serve output.zarr --port 8090
```

`info`, `query`, and `tile` also accept `--url <URL>` instead of a positional local store. HTTPS and loopback HTTP are supported for trusted read-only stores; upstream HTTP behavior has no total-timeout, request/byte-budget, full-body avoidance, or HTTPS-downgrade guarantee.

Conversion requires GDAL 3.13+ (`GDAL_BIN_DIR` optionally selects its executable directory). Existing store and tile outputs are never overwritten. Empty tiles create no file. PNG output is quantized for display rather than lossless scientific data.

`serve` owns both the loopback nested-object range server and the embedded MapLibre 4.7.1 viewer. It serves real Rust-rendered tiles and all vendored assets, including the MapLibre/Mapbox/glfx/d3 license notices. The `example/` directory only launches and tests this same binary.

See the [workspace README](https://github.com/afterrealism/zarra#readme) for query layout, band/alpha semantics, the exact support matrix, HTTP limitations, and all verification commands.

## Verification and license

Run `cargo test -p zarra-cli --all-features --locked` from the workspace. `bash scripts/test-gdal.sh` additionally runs the real GDAL 3.13.1 conversion suite in Docker, and `npm --prefix example test` builds this binary for the Node 20+ server/viewer integration tests.

Licensed under Apache-2.0 OR MIT. See the workspace `LICENSE-APACHE`, `LICENSE-MIT`, and `ATTRIBUTION.md` files.
