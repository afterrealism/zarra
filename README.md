# Zarra

Zarra converts georeferenced GeoTIFF/COG rasters into canonical multiscale Zarr v3 directories, reads their sharded arrays locally or over keyed HTTP, runs read-only SQLite queries over bounded raster windows, renders XYZ RGBA PNG tiles, and serves an embedded MapLibre viewer with local GeoTIFF uploads. The standalone `zarra-wasm` artifact reuses the async store, renderer, and SQL core for direct signed-object-store browser sessions. The workspace contains the `zarra` library, the `zarra-cli` package (which installs a binary named `zarra`), and `zarra-wasm`.

Zarra uses the upstream Zarr format; it is not a new single-file raster format. The supported profile is defined in [SPEC.md](SPEC.md).

## Package status

Version 0.1.0 is **Unreleased**. Local and path use is supported. crates.io publication is deferred until the `zarrs 0.23.15` upstream gate in [RELEASING.md](RELEASING.md) succeeds; the full-SHA backport used by this checkout is not a registry release.

The intended destinations after publication are [docs.rs/zarra](https://docs.rs/zarra), [crates.io/crates/zarra](https://crates.io/crates/zarra), and [crates.io/crates/zarra-cli](https://crates.io/crates/zarra-cli). These links are destinations, not claims that the packages have been published. Use the path installation below now.

## Quickstart

Install Rust 1.91.0 or newer. Conversion also requires the GDAL 3.13+ command-line tools; GDAL 3.13.1 is the tested compatibility version. Then, from the repository root:

```sh
cargo install --path crates/zarra-cli
zarra convert input.tif -o output.zarr --resampling bilinear --zstd-level 5
zarra info output.zarr
zarra query output.zarr --level 0 --x 0 --y 0 --width 8 --height 8
zarra sql output.zarr "SELECT band, AVG(value) FROM raster GROUP BY band"
zarra tile output.zarr --z 0 --x 0 --y 0 -o tile.png
zarra serve output.zarr --port 8090 --upload-root uploads
# or, for the committed reference fixture:
npm --prefix example start
```

`zarra serve` prints its loopback URL. The example launcher serves `testdata/refpy.zarr` and enables no-overwrite uploads under `example/data/uploads`; it does not implement another server or decoder.

## Commands

```text
zarra convert <INPUT.tif> -o <OUTPUT.zarr> [--maxzoom <Z>] \
  [--resampling <auto|nearest|bilinear|cubic|average>] [--zstd-level <1-22>]
zarra info <STORE> [--url <URL>]
zarra query <STORE> [--url <URL>] --level <Z> --x <X0> --y <Y0> \
  --width <W> --height <H> [--bands <0,2,1>]
zarra sql <STORE> [--url <URL>] <SQL> [--level <Z>] [--x <X0> --y <Y0>] \
  [--width <W> --height <H>]
zarra tile <STORE> [--url <URL>] --z <Z> --x <X> --y <Y> \
  [--bands <0,2,1>] -o <TILE.png>
zarra serve <STORE> [--port <PORT>] [--upload-root <DIRECTORY>]
```

- `convert` probes a local raster, builds every zoom in a sibling temporary directory, validates the store, and publishes it with one local directory rename. `--maxzoom` overrides native-zoom detection, `--resampling` selects the GDAL method (`auto` preserves the previous categorical/bilinear behavior), and `--zstd-level` accepts 1–22 (default 5).
- `info` prints the root manifest and metadata for every level.
- `query` writes an array-relative pixel window as JSON.
- `sql` executes one read-only SQLite statement over `raster(x, y, band, value)` and writes bounded JSON rows.
- `tile` renders one native or overzoomed XYZ tile as RGBA8 PNG.
- `serve` binds `127.0.0.1` and provides tiles, manifest JSON, nested-object byte ranges, and the embedded viewer. When `--upload-root` is supplied, the viewer can convert uploaded GeoTIFFs into that directory and switch the active store. It is a sequential demonstration server, not a production service. It accepts GET/HEAD/POST/OPTIONS, caps request heads at 16 KiB, upload bodies at 512 MiB, SQL bodies at 64 KiB, and applies 30-second per-connection I/O timeouts. Long conversion blocks other requests.

`convert` and `serve` are local-only. For `info`, `query`, `sql`, and `tile`, supply exactly one positional store or `--url`; supplying both or neither is an error.

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

## SQL raster analytics

`zarra sql` and the viewer's **SQL query** workspace expose one table per query:

```text
raster(x INTEGER, y INTEGER, band INTEGER, value REAL)
```

Coordinates are array-relative and bands are zero-based. NaN and declared nodata samples are loaded as `NULL`; use `COUNT(value)` for valid samples and `COUNT(*) - COUNT(value)` for missing samples. For example:

```sql
SELECT band, COUNT(value) AS valid, MIN(value), MAX(value), AVG(value)
FROM raster GROUP BY band ORDER BY band;
```

The highest store level is the default. Omit both `--width` and `--height` for the whole level, or set both for a window; the viewer offers the same controls. Each query stages at most 1,048,576 band values in a fresh in-memory SQLite database. It permits one read-only statement, interrupts execution after 10 seconds, and returns at most 1,000 rows in an 8 MiB compact JSON body (including metadata and escaping). This is on-demand window analytics, not a persistent analytics sidecar or a second raster decoder.

## Standalone browser WASM

`crates/zarra-wasm` is the actual Rust/WASM artifact for Tilify's direct data
plane. It uses `default-features=false, features=["async","sql"]`, fetches each
signed Zarr key directly from object storage, renders PNG through the shared
renderer, and executes the same bounded read-only SQLite implementation. Build
it with Rust 1.91 and wasm-pack 0.15.1:

```sh
npx --yes wasm-pack@0.15.1 build crates/zarra-wasm --target web --release -- --locked
```

Tilify's C++ app only processes uploads and signs registry-scoped GET URLs; it
does not render uploaded tiles or execute uploaded-data SQL. Browser signing
refreshes at `min(60s,20% of TTL)` before expiry, retries one 401/403, and
rejects generation changes. The bucket must allow the exact app origins,
GET + Range, and expose `Accept-Ranges`, `Content-Length`, and `Content-Range`.
Partial reads require exact 206 responses; there is no whole-object fallback.

The native CLI and `zarra serve` demo remain supported and are separate from
this browser artifact. See the [Tilify integration guide](docs/tilify-integration.md)
for exact WASM/test commands, support limits, CORS/error semantics, the QGIS
1 GiB local-cache/GDAL 3.13+ contract, deletion lifecycle, and verification.

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
| Storage | Local directories; optional keyed HTTP reads; no-overwrite local conversion uploads; async browser reads through caller-supplied signed URLs | Native signing/minting, remote uploads/publication, authentication, PostgreSQL, deletion, or cache invalidation as a multi-tenant service |
| Creation | GeoTIFF/COG through GDAL 3.13+ CLI; optional zoom, resampling, and ZSTD controls | Native Rust GeoTIFF decoding, reprojection, or resampling |
| Arrays | Zarr v3; 2D/3D; default `/` keys; little-endian bytes; optional inner ZSTD; CRC32C sharding | Arbitrary Zarr versions, layouts, endianness, codecs, dimensionality, or storage transformers |
| Values | `uint8`, `int8`, `uint16`, `int16`, `uint32`, `int32`, `float32`, `float64`; shared dtype/nodata | 64-bit integers, complex values, or mixed band dtypes/nodata |
| Query | Level metadata, chunk enumeration/reads, windows, neighborhoods, ordered/repeated bands, bounded read-only SQLite analytics | Persistent analytics sidecar, Arrow schema, or a separate `zarra-sql` crate |
| Rendering | XYZ zoom 0–22, native/overzoom, RG16/RGB8, first-band nodata alpha, 200 PNG/204 empty | Terrain color, hillshade, live rescaling, GPU shaders, or lossless scientific PNG values |
| Viewer | Native loopback MapLibre 4.7.1 viewer plus standalone browser `zarra:///{z}/{x}/{y}.png` protocol, extent fit, PNG rendering, and SQL results | deck.gl, basemaps, accounts, cloud analytics, or a second JavaScript decoder |
| Package | Core, CLI, embedded viewer, standalone `zarra-wasm`, npm launcher/tests, docs and CI | Native QGIS integration, production hosting, a Pages site, an xtask crate, or a multi-tenant service |

The fixture-based tests are not large-raster benchmarks, and no performance, remote byte-budget, LRU-cache, or distributed atomicity claim is made. Zarra does not impose a total GDAL CPU, memory, disk, or time bound; its checked chunk/query allocation caps are not total process-memory limits.

## Verification

Ordinary Rust tests do not require host GDAL. Run the complete local/path checks from the repository root:

```sh
bash scripts/verify-fixtures.sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features --locked
cargo check -p zarra --no-default-features
cargo test -p zarra-wasm --locked
cargo clippy -p zarra-wasm --target wasm32-unknown-unknown --locked -- -D warnings
npx --yes wasm-pack@0.15.1 build crates/zarra-wasm --target web --release -- --locked
npx --yes wasm-pack@0.15.1 test --headless --chrome crates/zarra-wasm -- --locked
node --test crates/zarra-wasm/tests/browser.test.mjs
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

Node.js 20+ is required for the example and direct browser tests. The example tests build the real CLI and exercise the same `geo.js` module used by the native viewer; the `zarra-wasm` tests separately build and exercise the actual standalone artifact with signed range/error/expiry, PNG, and SQL assertions. The GDAL script requires Docker and runs the ignored 2D/3D conversion, custom conversion options, HTTP upload/active-store conversion, reopen/query, rendering, and no-clobber integration tests against GDAL 3.13.1 and Rust 1.91.0.

`cargo package --list` checks file inclusion only. It is not publish readiness: actual packaging and publication remain gated by upstream `zarrs 0.23.15` availability. See [RELEASING.md](RELEASING.md).

## License and provenance

Licensed under either [Apache License 2.0](LICENSE-APACHE) or [MIT License](LICENSE-MIT), at your option. Unless you explicitly state otherwise, contributions are dual-licensed the same way without additional terms.

See [ATTRIBUTION.md](ATTRIBUTION.md) for upstream behavior, fixtures, libraries, and vendored notices, and [CHANGELOG.md](CHANGELOG.md) for the 0.1.0 release status.
