# Zarra raster example

`zarra serve` owns both the loopback range server and the embedded MapLibre viewer. This directory contains only the npm launcher, integration tests, and this documentation; it has no second server or JavaScript Zarr decoder.

## Run

From the repository root:

```sh
npm --prefix example start
```

Open the loopback URL printed by the CLI. The launcher serves `testdata/refpy.zarr` on port 8090 and requires Cargo, but no npm dependencies.

For optional rebuild-and-restart development, install `cargo-watch` yourself and run `npm --prefix example dev`. It is not installed automatically.

## Test

```sh
npm --prefix example test
```

The test command builds `target/debug/zarra`, starts real servers with `--port 0`, checks the embedded assets and HTTP/tile/range behavior, imports the same `geo.js` used by the browser, and kills and reaps its server processes. Node.js 20 or newer is required.
