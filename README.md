# Zarra

Zarra converts georeferenced GeoTIFF/COG rasters into canonical multiscale Zarr v3 directories, reads their sharded arrays locally or over keyed HTTP, renders XYZ RGBA PNG tiles, and serves an embedded MapLibre viewer. The workspace contains the `zarra` library and the `zarra-cli` package, which installs a binary named `zarra`.

Zarra uses the upstream Zarr format; it is not a new single-file raster format. The supported profile is defined in [SPEC.md](SPEC.md).

## Package status

Version 0.1.0 is **Unreleased**. Local and path use is supported. crates.io publication is deferred until the `zarrs 0.23.15` upstream gate in [RELEASING.md](RELEASING.md) succeeds; the full-SHA backport used by this checkout is not a registry release.

The intended destinations after publication are [docs.rs/zarra](https://docs.rs/zarra), [crates.io/crates/zarra](https://crates.io/crates/zarra), and [crates.io/crates/zarra-cli](https://crates.io/crates/zarra-cli). These links are destinations, not claims that the packages have been published. Use the path installation below now.

## Quickstart

Install Rust 1.91.0 or newer. Conversion also requires the GDAL 3.13+ command-line tools; GDAL 3.13.1 is the tested compatibility version. Then, from the repository root:

```sh
cargo install --path crates/zarra-cli
zarra convert input.tif -o output.zarr
zarra info output.zarr
zarra query output.zarr --level 0 --x 0 --y 0 --width 8 --height 8
zarra tile output.zarr --z 0 --x 0 --y 0 -o tile.png
zarra serve output.zarr --port 8090
# or, for the committed reference fixture:
npm --prefix example start
```

`zarra serve` prints its loopback URL. The example launcher serves `testdata/refpy.zarr`; it does not implement another server or decoder.

## Commands

```text
zarra convert <INPUT.tif> -o <OUTPUT.zarr>
zarra info <STORE> [--url <URL>]
zarra query <STORE> [--url <URL>] --level <Z> --x <X0> --y <Y0> \
  --width <W> --height <H> [--bands <0,2,1>]
zarra tile <STORE> [--url <URL>] --z <Z> --x <X> --y <Y> \
  [--bands <0,2,1>] -o <TILE.png>
zarra serve <STORE> [--port <PORT>]
```

- `convert` probes a local raster, builds every zoom in a sibling temporary directory, validates the store, and publishes it with one local directory rename.
- `info` prints the root manifest and metadata for every level.
- `query` writes an array-relative pixel window as JSON.
- `tile` renders one native or overzoomed XYZ tile as RGBA8 PNG.
- `serve` binds `127.0.0.1` and provides tiles, manifest JSON, nested-object byte ranges, and the embedded viewer. It is a sequential demonstration server, not a production service. It accepts GET/HEAD/OPTIONS, caps request heads at 16 KiB, and applies 30-second per-connection I/O timeouts.

`convert` and `serve` are local-only. For `info`, `query`, and `tile`, supply exactly one positional store or `--url`; supplying both or neither is an error.

### Optional HTTP stores

For example:

```sh
zarra info --url https://example.com/raster.zarr
zarra query --url https://example.com/raster.zarr \
  --level 0 --x 12 --y 12 --width 8 --height 8
zarra tile --url https://example.com/raster.zarr \
  --z 13 --x 0 --y 0 -o remote-tile.png
```

HTTPS is accepted. Plain HTTP is restricted to `localhost`, `127.0.0.0/8`, and `[::1]`. URL userinfo, query strings, fragments, and other schemes are rejected before network traffic. Keys are appended to the supplied base URL, and only single-part ranges are requested.

HTTP is intended for trusted, read-only public stores. The upstream `zarrs_http` client can accept a full-body 200 response for some range requests and exposes no total-timeout setting. Redirect handling is the upstream `reqwest` default, with no HTTPS-downgrade guarantee. Zarra therefore makes no remote request-count, transferred-byte, redirect-policy, or total-timeout guarantee. HTTP reading does not provide remote conversion or publication.

### GDAL conversion

`convert` requires `gdalinfo`, `gdalwarp`, and `gdal_translate` from GDAL 3.13 or newer. Set `GDAL_BIN_DIR` to the directory containing those executables when they are not on `PATH`:

```sh
GDAL_BIN_DIR=/opt/gdal/bin zarra convert input.tif -o output.zarr
```

The tools are invoked directly with argument arrays, never through a shell, and Zarra does not link libgdal. Input must be a nonempty, georeferenced local GeoTIFF/COG with supported, consistent band metadata. Conversion creates one-band 2D arrays and multiband 3D arrays aligned to WebMercatorQuad; readers also accept a one-band array with an explicit `[1,H,W]` band axis. The output path must not exist. A second conversion to the same path fails without replacing the first store. Temporary build content is cleaned on failure; no remote or multi-object atomic publication is claimed.

## Store and query model

A store is a Zarr v3 directory or key prefix:

```text
output.zarr/
├── zarr.json          # root group with attributes.zarra
├── 0/
│   ├── zarr.json      # canonical sharded array
│   └── c/...          # shard objects
└── ... through maxzoom
```

Chunk coordinates and query origins are array-relative. `--x` and `--y` may be negative, and windows may extend beyond the array. Outside-array cells and genuinely sparse chunks produce NaN; metadata, transport, and decode failures remain errors rather than becoming NaN.

Query JSON contains `level`, `x`, `y`, `width`, `height`, ordered `bands`, and `values`. Values are row-major and band-interleaved:

```text
index = (row * width + column) * bands.length + channel
```

The result has `width * height * bands.length` values. `--bands 0,2,1` selects that order, repeated indices are allowed, and each index may have a `b`/`B` prefix. Without `--bands`, every band is returned in source order. Non-finite samples are valid JSON strings: `"NaN"`, `"Infinity"`, or `"-Infinity"`.

The library also exposes full padded inner-chunk reads, chunk enumeration, band-planar chunk values, and checked neighborhood reads. Query results use `f64`; PNG rendering is a separate display operation.

## Tile encoding and output safety

An empty tile band selection uses band 0 for one-band data or the first up-to-three bands for multiband data. Explicit selections contain one to three indices and may repeat.

- **One selected band — RG16:** clamp to that band's display range, round to 16 bits, and store the high byte in R and low byte in G. B is 0.
- **Two or three selected bands — RGB8:** independently clamp, normalize, and round each band to 8-bit R/G/B. With two bands, B repeats the first selected band. There is no fourth-band or alpha-band interpretation.
- **Transparency:** only the first selected band controls declared-nodata transparency. NaN in that band is always transparent; later bands do not clear alpha. Transparent pixels are zero-filled.

Native and deep-overzoom tiles always read the appropriate maxzoom ancestor. PNG samples are clamped and quantized for display; they are not lossless scientific values even though PNG compression itself is lossless.

`tile` creates its output with no-clobber semantics and never replaces an existing file. An empty tile prints `empty` and creates no output file.

## Support matrix and non-goals

| Area | Included in 0.1.0 | Not included |
|---|---|---|
| Storage | Local directories; optional keyed HTTP reads | Signed S3/SigV4, uploads, authentication, PostgreSQL, deletion, cache invalidation, or remote publication |
| Creation | GeoTIFF/COG through GDAL 3.13+ CLI | Native Rust GeoTIFF decoding, reprojection, or resampling |
| Arrays | Zarr v3; 2D/3D; default `/` keys; little-endian bytes; optional inner ZSTD; CRC32C sharding | Arbitrary Zarr versions, layouts, endianness, codecs, dimensionality, or storage transformers |
| Values | `uint8`, `int8`, `uint16`, `int16`, `uint32`, `int32`, `float32`, `float64`; shared dtype/nodata | 64-bit integers, complex values, or mixed band dtypes/nodata |
| Query | Level metadata, chunk enumeration/reads, windows, neighborhoods, ordered/repeated bands | SQL, an analytics sidecar, Arrow schema, or a `zarra-sql` crate |
| Rendering | XYZ zoom 0–22, native/overzoom, RG16/RGB8, first-band nodata alpha, 200 PNG/204 empty | Terrain color, hillshade, live rescaling, GPU shaders, or lossless scientific PNG values |
| Viewer | MapLibre 4.7.1 raster source at opacity 0.92, extent fit, navigation, metadata and XYZ telemetry | deck.gl, basemaps, accounts, analytics, uploads, or a second JavaScript decoder |
| Package | Core, CLI, embedded viewer, npm launcher/tests, docs and CI | WASM bindings, QGIS integration, production hosting, a Pages site, an xtask crate, or a multi-tenant service |

The fixture-based tests are not large-raster benchmarks, and no performance, remote byte-budget, LRU-cache, or distributed atomicity claim is made. Zarra does not impose a total GDAL CPU, memory, disk, or time bound; its checked chunk/query allocation caps are not total process-memory limits.

## Verification

Ordinary Rust tests do not require host GDAL. Run the complete local/path checks from the repository root:

```sh
bash scripts/verify-fixtures.sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features --locked
cargo check -p zarra --no-default-features
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps
cargo +1.91.0 test --workspace --all-features --locked
npm --prefix example test
install_root="$(mktemp -d)"
cargo install --path crates/zarra-cli --root "$install_root"
"$install_root/bin/zarra" --help
rm -rf "${install_root:?}"
cargo package -p zarra --list --allow-dirty
cargo package -p zarra-cli --list --allow-dirty
bash scripts/test-gdal.sh
```

Node.js 20+ is required for the example tests, which build the real CLI and exercise the same `geo.js` module used by the browser. The GDAL script requires Docker and runs the ignored 2D/3D conversion, reopen/query, rendering, and no-clobber integration tests against GDAL 3.13.1 and Rust 1.91.0.

`cargo package --list` checks file inclusion only. It is not publish readiness: actual packaging and publication remain gated by upstream `zarrs 0.23.15` availability. See [RELEASING.md](RELEASING.md).

## License and provenance

Licensed under either [Apache License 2.0](LICENSE-APACHE) or [MIT License](LICENSE-MIT), at your option. Unless you explicitly state otherwise, contributions are dual-licensed the same way without additional terms.

See [ATTRIBUTION.md](ATTRIBUTION.md) for upstream behavior, fixtures, libraries, and vendored notices, and [CHANGELOG.md](CHANGELOG.md) for the 0.1.0 release status.
