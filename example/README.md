# Zarra raster example

`zarra serve` owns both the loopback range server and the embedded MapLibre viewer. This directory contains only the npm launcher, integration tests, and this documentation; it has no second server or JavaScript Zarr decoder.

## Run

From the repository root:

```sh
npm --prefix example start
```

Open the loopback URL printed by the CLI. The launcher serves `testdata/refpy.zarr` on port 8090 and enables no-overwrite conversion outputs in `example/data/uploads`. It requires Cargo and GDAL 3.13+ for conversion, but has no npm dependencies.

## Upload and query

- **Upload GeoTIFF** accepts one georeferenced `.tif`/`.tiff` file. Choose a new `.zarr` output name, optional maximum zoom, resampling method, and ZSTD level, then convert and open the new active store. Upload bodies are capped at 512 MiB and existing outputs are never replaced.
- **SQL query** runs one read-only SQLite statement over `raster(x, y, band, value)` for the active store. Select a level and optional pixel window; leave both dimensions blank for the whole level. NaN and declared nodata values appear as `NULL`, and example buttons cover previews, band statistics, missing samples, and high values.

Both workflows use the same Rust server and Zarra reader as the map. There is no JavaScript Zarr decoder or second application server.

For optional rebuild-and-restart development, install `cargo-watch` yourself and run `npm --prefix example dev`. It is not installed automatically.

## Test

```sh
npm --prefix example test
```

The test command builds `target/debug/zarra`, starts real servers with `--port 0`, checks the embedded assets and HTTP/tile/range/SQL behavior, imports the same `geo.js` used by the browser, and kills and reaps its server processes. Node.js 20 or newer is required. The separate `bash scripts/test-gdal.sh` suite covers real upload conversion in the GDAL 3.13.1 container.
