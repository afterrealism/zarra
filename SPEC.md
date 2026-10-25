# Zarra Raster Profile and Package Specification

**Status:** Draft v1, 2026-10-25  
**Package:** `zarra` / `zarra-cli` 0.1.0  
**Upstream format:** Zarr v3, including the `sharding_indexed` codec

This document specifies the small geospatial raster profile and package behavior ported from Tilify Everything. It is not a replacement for the Zarr specification. The authoritative format references are:

- [Zarr v3 core specification](https://zarr-specs.readthedocs.io/en/latest/v3/core/index.html)
- [ZEP0002: sharding-indexed storage](https://zarr.dev/zeps/accepted/ZEP0002.html)
- [RFC 9110 §14: range requests](https://www.rfc-editor.org/rfc/rfc9110.html#section-14)
- [MapLibre GL JS 4.7.1 raster sources](https://maplibre.org/maplibre-gl-js/docs/)

## 1. Goal and observable completion

The package must provide one coherent local workflow:

```text
GeoTIFF/COG
  → GDAL-probed, WebMercatorQuad-aligned multiscale Zarr v3 directory
  → local or HTTP chunk/window/band queries
  → XYZ RGBA8 PNG tiles
  → loopback MapLibre raster viewer
```

It is complete when all of these are demonstrated by automated checks:

1. `zarra convert` turns the committed COG into a reopened store with one-band 2D arrays and a derived three-band COG into 3D arrays.
2. Local and HTTP queries return the independent `zarr-python` fixture's exact `uint16` values, including offsets, cross-chunk windows, NaN padding, and sparse chunks.
3. Golden tests verify RG16, RGB8, nodata transparency, band selection, XYZ offsets, and overzoom through `maxzoom + 13` and zoom 22.
4. `zarra serve` returns tiles, metadata, static viewer assets, and exact nested-object byte ranges with safe error responses.
5. The viewer displays the real tile endpoint in MapLibre and fits the declared extent; it does not use a hard-coded image.
6. Formatting, Clippy, workspace tests, Rust 1.91.0 locked tests, GDAL 3.13.1 integration tests, and Node 20+ viewer tests pass.

## 2. Scope

### Included

- A `zarra` Rust library for conversion, local/HTTP querying, and tile rendering.
- A `zarra` binary from `zarra-cli` with `convert`, `info`, `query`, `tile`, and `serve` commands.
- Local directory stores and read-only keyed HTTP stores through an optional `http` feature.
- Zarr v3 arrays in the canonical raster layout below, using maintained `zarrs` decoding rather than a new codec implementation.
- GeoTIFF/COG conversion through GDAL CLI 3.13 or newer; GDAL 3.13.1 is the compatibility-test version.
- A read-only loopback server and a small MapLibre 4.7.1 viewer.
- Dual Apache-2.0/MIT packaging, documentation, fixture-based tests, and stable/MSRV/GDAL CI.

### Explicit non-goals

- SQL or a `zarra-sql` crate. Tilify has array reads, not raster SQL.
- A new binary format, EOF footer, A5 aggregate, FlatGeobuf body, or Arrow sidecar.
- Native Rust reprojection, resampling, or GeoTIFF decoding in v1.
- Arbitrary Zarr versions, chunk-key encodings, endianness, codecs, dimensionality, or storage transformers.
- Signed S3/SigV4, uploads, authentication, PostgreSQL, deletion, cache invalidation, QGIS integration, production hosting, or a public multi-tenant service.
- Rust/WASM bindings or a second JavaScript Zarr decoder.
- deck.gl, terrain colormaps, hillshade, live rescaling, or GPU shaders. The source viewer uses an ordinary MapLibre raster layer; those other capabilities are not implemented there.
- Lossless scientific values in PNG. PNG bytes are losslessly compressed, but RG16/RGB8 encoding clamps and quantizes source values.

## 3. Package and dependency contract

The Cargo workspace uses resolver 2, edition 2021, version 0.1.0, MSRV 1.91.0, and `Apache-2.0 OR MIT`. The minimum dependency baseline is:

| Dependency | Version / features | Responsibility |
|---|---|---|
| `zarrs` | `0.23.15` full-SHA backport `9e55886ecfc4e8f282632d6fb7682869ca69db4b`; default features off; `filesystem,sharding,crc32c,zstd` | Zarr metadata, keyed storage, sharded reads, codecs |
| `zarrs_http` | `=0.3.1`; optional under `zarra/http` | Synchronous keyed HTTP storage |
| `url` | 2; optional under `zarra/http` | Pre-network URL scheme and loopback validation |
| `serde` / `serde_json` | 1 | Root manifest and GDAL JSON |
| `thiserror` | 2 | Typed library errors |
| `png` | `=0.18.1` | RGBA8 PNG encoding and golden-test decoding |
| `tempfile` | 3 | Same-parent build/test temporary directories |
| `clap` | 4 with `derive`; CLI only | Command-line parsing |

The `zarrs 0.23.15` backport declares Rust 1.91, which sets the workspace MSRV. It fixes the 0.23.14 sparse shard-index overflow found by real GDAL conversion. The root workspace also pins `zarrs_storage` to the same full SHA so registry `zarrs_http 0.3.1` uses a compatible storage trait source. `ndarray`, Blosc, gzip, transpose, and other default `zarrs` features remain disabled. Application crates use `#![forbid(unsafe_code)]`; workspace Clippy denies `unwrap_used`, `expect_used`, `panic`, `unimplemented`, `todo`, and `unreachable` in application code.

**Release gate:** crates.io has only `zarrs 0.23.14` as of 2026-10-25. The user approved keeping the full-SHA backport for local/path use and deferring crates.io publication until `0.23.15` is released. `cargo package --list` is only a file-inclusion check, not a publish-readiness claim. Actual `cargo package`/`cargo publish` remains blocked until `cargo info zarrs@0.23.15` succeeds and the complete package verification passes.

`zarrs_http` accepts a full-body 200 response to some range requests and does not expose its client's total timeout. Therefore v1 makes no remote request-count, byte-budget, or timeout guarantee. HTTP is optional and intended for trusted/read-only public stores; the local conversion and demo paths do not depend on it.

## 4. Store and root manifest

A store is a directory or key prefix, never a single FlatGeobuf-style file. Its layout is:

```text
<store>/
├── zarr.json                 # Zarr v3 group; attributes.zarra is the manifest
├── 0/
│   ├── zarr.json             # canonical array metadata
│   └── c/...                 # shard objects
├── 1/
│   ├── zarr.json
│   └── c/...
└── … up to maxzoom
```

Each level contains only the pixel array. GDAL's `<level>/data/*` output is flattened to `<level>/*`; its X/Y coordinate arrays, per-level group metadata, and warp intermediates are not published. The root group metadata is retained and augmented.

The root group's `attributes.zarra` value has this shape:

```json
{
  "profile_version": 1,
  "crs": 3857,
  "tile_size": 256,
  "dtype": "uint16",
  "bands": 1,
  "nodata": 65535,
  "minzoom": 0,
  "maxzoom": 1,
  "levels": [
    { "z": 0, "xoff": 0, "yoff": 0 },
    { "z": 1, "xoff": 0, "yoff": 0 }
  ],
  "tile_encoding": {
    "channels": "rg16",
    "encoded_min": 0.0,
    "encoded_max": 7000.0
  },
  "per_band_range": [[0.0, 7000.0]],
  "extent": [-180.0, -85.0, 180.0, 85.0]
}
```

Rules:

- `profile_version` is 1 and `crs` is 3857.
- `tile_size` is a positive square spatial inner-chunk edge. GDAL conversion writes 256; the independent query fixture declares 16 so it can also exercise the complete local viewer.
- `dtype` is one of `uint8`, `int8`, `uint16`, `int16`, `uint32`, `int32`, `float32`, or `float64`. Complex and 64-bit integer types are rejected rather than decoded incorrectly.
- `bands` is at least 1. `nodata` is `null`, one finite number, or the string `"NaN"`; NaN is valid only for floating-point data.
- `minzoom` is 0, `maxzoom` is 0–22, and `levels` contains exactly one entry for every zoom in that inclusive range. `xoff`/`yoff` are the world-tile coordinates represented by array chunk `(0,0)` at that level and must each be less than `2^z`.
- `channels` is `rg16` for a one-band default or `rgba8` for a multi-band default. `encoded_max > encoded_min`, `[encoded_min,encoded_max]` equals `per_band_range[0]`, and `per_band_range` has one finite `[min,max]` pair per band with `max > min`.
- `extent` is optional `[west,south,east,north]`, with `west <= east`, `south <= north`, longitude in `[-180,180]`, and latitude in `[-90,90]`. The viewer uses it only to fit the map.
- Fields generated but unused by the source consumer—`default_rescale`, `default_colormap`, `hillshade`, `render_mode`, ownership, upload state, and source object keys—are not part of this profile.

`RasterManifest::validate` checks all rules before array data is read. Opening a store also validates the maximum-zoom array so mismatches fail early.

## 5. Conversion contract

### 5.1 Input and process boundary

`convert` accepts a nonempty local GeoTIFF/COG path and an output directory that does not exist. It rejects input without a CRS/GCP georeference, missing bands, mixed band dtypes, conflicting per-band nodata declarations, unsupported dtypes, non-finite geometry/statistics, and failed GDAL commands. It never invokes a shell. `GDAL_BIN_DIR`, when nonempty, selects the directory containing `gdalinfo`, `gdalwarp`, and `gdal_translate`; otherwise the tools are resolved on `PATH`.

The implementation uses `std::process::Command::output` and includes tool name, exit status, and bounded stderr in errors. Source paths and numeric parameters are individual arguments, not command text.

### 5.2 Probe

1. Run `gdalinfo -json -approx_stats <source>` (do not also pass `-stats`).
2. Read band count/types/nodata/palette, approximate per-band ranges, and WGS84 extent. Every band must have the first band's dtype. All declared band nodata values must agree; absent declarations on every band are handled by the synthesized-fill rule below, and mixed present/absent declarations are rejected because the root profile has one shared value.
3. In the build temporary directory, run `gdalwarp -t_srs EPSG:3857 -of VRT -overwrite <source> <probe.vrt>` and then `gdalinfo -json <probe.vrt>`.
4. Read the 3857 corner coordinates and absolute X pixel size. The VRT is a probe only; no pixels are published from it.

Approximate statistics are used only for display scaling and nodata selection, not scientific queries. When a band has no usable range, its fallback is `[minimum, minimum + 1]` as in the source pipeline.

### 5.3 WebMercatorQuad planning

Use `WORLD = 20037508.342789244`, `SPAN = 2 * WORLD`, and 256-pixel tiles:

```text
resolution(z) = SPAN / (256 * 2^z)
tile_span(z)  = SPAN / 2^z
maxzoom       = clamp(ceil(log2(SPAN / (256 * native_pixel_meters))), 0, 22)
```

Snap each level's 3857 AOI to inclusive tile indices with the source's `1e-9` upper-edge guard, then derive the exact tile-aligned warp extent. Levels are generated from `maxzoom` down to 0. Palette/thematic input uses nearest-neighbor resampling; other input uses bilinear.

Propagate source nodata. If it is absent, floating-point output uses NaN. Integer output prefers 0 when outside every band's observed range; otherwise choose the nearest representable integer below all ranges or above all ranges. If the values span the whole dtype and no fill remains, continue without a declared fill rather than inventing one.

### 5.4 GDAL array emission and publication

For each level:

```text
gdalwarp -t_srs EPSG:3857 -r <near|bilinear>
         -tr <resolution> <resolution>
         -te <minx> <miny> <maxx> <maxy> -te_srs EPSG:3857
         [-dstnodata <value|nan>] -overwrite <source> <level.tif>

gdal_translate -of ZARR -co FORMAT=ZARR_V3 -co ARRAY_NAME=data
               -co BLOCKSIZE=<outer-shape>
               -co SHARD_CHUNK_SHAPE=<inner-shape>
               -co COMPRESS=ZSTD -co ZSTD_LEVEL=5
               <level.tif> <build>/<level>
```

Outer spatial edges are `min(2048, touched_tiles * 256)`, rounded by construction to a multiple of 256. Inner spatial shape is 256×256. A one-band array uses two dimensions `[H,W]`; multiple bands use `[bands,H,W]`. The band axis is included in both shapes only for 3D arrays, so one inner chunk contains every band.

GDAL may emit `fill_value: null` for multiband arrays. Before publication, a null or missing `fill_value` is normalized to the manifest nodata value, or to 0 when no nodata is declared. Integer fills must be finite, integral, and representable in the level dtype; float NaN is written as the string `"NaN"`. This is required for `zarrs` to reopen converted stores; sparse detection still comes from shard indexes, not fill-value comparison.

The store is built as `store` inside a fresh sibling temporary directory. After all levels and the root manifest validate, `fs::rename` publishes that directory in one operation. The output is never overwritten; failure removes temporary content. There is no claim of atomicity across separate object-storage uploads because v1 publishes only a local directory.

`BuildReport` returns the output path, validated manifest, number of published levels, and number of published array/shard files.

## 6. Canonical array validation

Every opened level must satisfy all of the following:

- Zarr v3 `array`, regular chunk grid, default chunk-key encoding with `/` separator, no storage transformers.
- Two dimensions `[H,W]` or three `[bands,H,W]`; positive sizes. One-band arrays may use either form for read compatibility, including the independent `[1,64,64]` fixture; conversion writes 2D for one band. Every 3D band shape matches the root manifest.
- One of the eight profile dtypes, little-endian `bytes` encoding, and the root manifest's dtype.
- One outer `sharding_indexed` codec. Inner codecs are `bytes` followed by optional `zstd`; no other array/byte codec is accepted.
- Shard index codecs are `bytes` plus `crc32c`; index location `end` (the writer's layout) or `start` (accepted by the reader).
- For 3D arrays, inner and outer band axes equal the band count; inner spatial edges are square and equal `tile_size`, and outer spatial edges are exact inner-edge multiples.

Checked arithmetic precedes allocation. A decoded chunk may contain at most 4,194,304 values and a query window at most 16,777,216 output values. `enumerate_chunks` applies the latter limit before allocating its coordinate vector. These are allocation guards, not claims about total process memory or GDAL resource use.

## 7. Query contract

The public types and signatures are:

```rust
pub struct RasterManifest {
    pub profile_version: u8,
    pub crs: u32,
    pub tile_size: u32,
    pub dtype: DType,
    pub bands: usize,
    pub nodata: Option<Nodata>,
    pub minzoom: u8,
    pub maxzoom: u8,
    pub levels: Vec<LevelOffset>,
    pub tile_encoding: TileEncoding,
    pub per_band_range: Vec<[f64; 2]>,
    pub extent: Option<[f64; 4]>,
}
pub struct LevelOffset { pub z: u8, pub xoff: u32, pub yoff: u32 }
pub struct TileEncoding {
    pub channels: Channels,
    pub encoded_min: f64,
    pub encoded_max: f64,
}
pub enum Channels { Rg16, Rgba8 }
pub enum Nodata { Number(f64), NaN }
pub enum DType { Uint8, Int8, Uint16, Int16, Uint32, Int32, Float32, Float64 }
pub struct LevelMeta {
    pub z: u8,
    pub bands: usize,
    pub width: u64,
    pub height: u64,
    pub inner: u32,
    pub dtype: DType,
}
pub struct ChunkRef { pub col: u64, pub row: u64 }
pub struct Chunk {
    pub bands: usize,
    pub width: u32,
    pub height: u32,
    pub values: Vec<f64>, // band-planar [band,row,col]
}
pub struct Window {
    pub width: u32,
    pub height: u32,
    pub bands: Vec<usize>,
    pub values: Vec<f64>, // row-major, band-interleaved
}

impl Chunk {
    pub fn value(&self, band: usize, row: u32, col: u32) -> Option<f64>;
}

impl RasterStore {
    pub fn open(path: &std::path::Path) -> Result<Self>;
    #[cfg(feature = "http")]
    pub fn open_url(base_url: &str) -> Result<Self>;
    pub fn manifest(&self) -> &RasterManifest;
    pub fn level_meta(&mut self, level: u8) -> Result<LevelMeta>;
    pub fn enumerate_chunks(&mut self, level: u8) -> Result<Vec<ChunkRef>>;
    pub fn read_chunk(&mut self, level: u8, col: u64, row: u64) -> Result<Option<Chunk>>;
    pub fn read_window(&mut self, level: u8, x0: i64, y0: i64, width: u32, height: u32) -> Result<Window>;
    pub fn read_bands(&mut self, level: u8, x0: i64, y0: i64, width: u32, height: u32, bands: &[usize]) -> Result<Window>;
    pub fn read_neighborhood(&mut self, level: u8, cx: i64, cy: i64, radius: u32) -> Result<Window>;
}
```

Every `Chunk` retains the full `inner × inner` shape, including padded edge chunks returned by `zarrs`. `Chunk::value` addresses that padded representation; window reads separately clip to the actual array bounds so out-of-array cells remain NaN.

Chunk coordinates are array-relative. Window coordinates are array-relative pixels and may extend outside the array. `read_chunk` returns `Ok(None)` for an out-of-grid or sparse chunk. Transport, metadata, and decode failures remain `Err`; they never silently become NaN.

`read_window` uses NaN only for out-of-array cells and genuinely sparse chunks. Its output is `f64`, avoiding the source's lossy `float` window buffer and preserving all profile dtypes exactly except floating-point NaN payload details. `read_bands` validates a nonempty ordered list of in-range zero-based indices and returns channels in the requested order; repeated indices are allowed for grayscale composites. `read_neighborhood` returns `(2r+1)^2` samples per selected band with NaN padding and checked radius arithmetic.

The implementation uses `ArrayShardedReadableExtCache` for shard indexes. Before a chunk read, sum cached shard counts across levels and clear all level caches when the total has reached 1024; one read may add only its shard. This is a store-wide reset budget, not an LRU or a cross-request compressed-chunk cache.

`open_url` accepts HTTPS, plus HTTP only for `localhost`, `127.0.0.0/8`, or `[::1]`; other schemes and non-loopback plain HTTP fail before network traffic. Userinfo, query strings, and fragments are rejected because keys are appended to the base URL. It calls `HTTPStore::set_batch_range_requests(false)` so servers receive only single-part ranges. Redirect policy is the upstream `reqwest` default; no HTTPS-downgrade guarantee is made in v1.

## 8. Tile-rendering contract

```rust
pub struct TileCoord { pub z: u8, pub x: u32, pub y: u32 }
pub enum Tile { Png(Vec<u8>), Empty }

pub fn render_tile(store: &mut RasterStore, tile: TileCoord, bands: &[usize]) -> Result<Tile>;
```

XYZ validation allows zoom 0–22 and requires `0 <= x,y < 2^z`. Empty `bands` selects the manifest default: the first band for one-band data or the first up-to-three bands for multi-band data. An explicit selection contains one to three valid zero-based indices; repeats are allowed. One selects RG16; two or three select RGB8. CLI/HTTP parsers accept an optional `b`/`B` prefix per token but reject empty, malformed, or out-of-range tokens instead of silently reverting to defaults.

For `z > maxzoom`, always read the maxzoom ancestor. Never copy Tilify's intermediate-level bug:

```text
k      = z - maxzoom
level  = maxzoom
factor = 2^k
ox     = x >> k
oy     = y >> k
winx   = x & (factor - 1)
winy   = y & (factor - 1)
```

Subtract that level's `xoff`/`yoff` with checked arithmetic; an out-of-grid or sparse chunk is `Empty`. Zoom 22 is supported without shifting overflow.

Packing is nearest-neighbor and produces an `inner × inner` RGBA8 buffer:

- RG16: clamp the selected first band to its encoding range, round to 16 bits, store high byte in R and low byte in G; B=0 and A=255.
- RGB8: independently clamp/normalize each selected band to 8-bit R/G/B; A=255. There is no fourth-band or alpha-band interpretation.
- Only the first selected band controls declared-nodata transparency, matching the source behavior. Numeric comparison uses `abs(value - nodata) <= 1e-9`; NaN values are always transparent, and declared NaN nodata therefore needs no special numeric comparison. Transparent pixels are zero-filled.
- If no pixel is painted, return `Empty`; otherwise encode color-type-6, 8-bit PNG through the `png` crate.

## 9. CLI contract

```sh
zarra convert <INPUT.tif> -o <OUTPUT.zarr>
zarra info <STORE> [--url <HTTPS_OR_LOOPBACK_HTTP_URL>]
zarra query <STORE> [--url <URL>] --level <Z> --x <X0> --y <Y0> \
  --width <W> --height <H> [--bands <0,2,1>]
zarra tile <STORE> [--url <URL>] --z <Z> --x <X> --y <Y> \
  [--bands <0,2,1>] -o <TILE.png>
zarra serve <STORE> [--port <PORT>]
```

A positional store and `--url` are mutually exclusive and one is required for `info`, `query`, and `tile`. `convert` and `serve` are local-only. Query JSON is streamed to stdout with level, x/y/width/height, ordered bands, and row-major band-interleaved values; non-finite samples are emitted as the strings `"NaN"`, `"Infinity"`, or `"-Infinity"`, never invalid JSON. `tile` writes a new file without clobbering an existing path and prints `empty` instead of creating a file for an empty tile. Errors produce one readable `error: …` line on stderr and a nonzero exit code.

`serve` binds `127.0.0.1`; port 0 is allowed for tests and the actual bound URL is printed. It is a sequential, one-request-per-connection demonstration server with 16 KiB request-head and 30-second per-connection I/O limits.

## 10. HTTP and viewer contract

The server supports GET, HEAD, and OPTIONS only:

| Route | Behavior |
|---|---|
| `/api/store` | JSON manifest plus `"tiles":"/raster/{z}/{x}/{y}.png"`; `Cache-Control: no-store` |
| `/raster/{z}/{x}/{y}.png[?bands=…]` | 200 `image/png`, 204 empty, 400 invalid coords/bands, 502 read/decode/encode error |
| `/store/<nested-key>` | Root manifest/array/shard bytes; full 200, single-range 206, HEAD, 404, 403 traversal, 416 unsatisfiable |
| `/`, `/index.html`, `/app.js`, `/geo.js`, `/styles.css`, `/favicon.svg` | Embedded viewer assets; no-cache |
| `/vendor/maplibre-gl.js`, `/vendor/maplibre-gl.css`, `/vendor/maplibre-LICENSE.txt` | Embedded pinned MapLibre 4.7.1 assets and license |

Successful tile and empty responses use `Cache-Control: public, max-age=3600, s-maxage=86400`. All supported routes send `Access-Control-Allow-Origin: *`; OPTIONS also allows GET/HEAD and the Range header, and exposes Accept-Ranges, Content-Length, and Content-Range. Raw object responses include `Accept-Ranges: bytes` and accurate `Content-Length`/`Content-Range`. Only one range is accepted; malformed ranges are ignored per the server helper's documented rule, and well-formed unsatisfiable ranges return 416.

Static assets are an explicit allowlist. Object keys are percent-decoded once; NUL, backslash, `.`/`..` escapes, absolute paths, non-files, and symlink escapes are rejected. No directory listing, upload, mutation, multipart range, or production hardening is claimed.

The viewer is served by `zarra serve`; `example/` contains only its npm launcher/tests and documentation, not a second server or decoder. It must:

- Fetch `/api/store` and register an ordinary MapLibre `{type:"raster"}` source using the real `/raster/{z}/{x}/{y}.png` template, declared `tile_size`, minzoom/maxzoom, and opacity 0.92.
- Fit the declared extent or a documented world fallback and provide MapLibre navigation plus a native fit button.
- Show dtype, bands, level range, current integer zoom and XYZ under the pointer, loading state, and readable errors.
- Use no basemap, analytics, account UI, upload UI, or fabricated request ledger.
- Use pinned local MapLibre JavaScript/CSS with its license, visible keyboard focus, labeled native controls, responsive layout, and `prefers-reduced-motion`.

## 11. Fixture and verification contract

The canonical fixtures are copied without changing their array bytes:

- `testdata/refpy.zarr/0/zarr.json` SHA-256 `a609e165541e8c062b7662564e6d64c4e5c6e94ecba914dc9202d041a2f66104`
- `testdata/refpy.zarr/0/c/0/0/0` SHA-256 `8e390da5ecd278c318d586f112d1484e7836047e22b0d487fbef32780b071e17`
- `testdata/sample.tif` SHA-256 `9b4604471f6a2bb5674ba990c28c3f51c26d1c201d849581225c968d25dd2664`

`refpy.zarr` was produced by `zarr-python 3.2.1`: shape `[1,64,64]`, `uint16`, inner 16, one 4×4-inner-chunk shard, and `value(y,x) = (y*1000+x) & 0xffff`. Add only a root group `zarr.json` containing the v1 manifest with `tile_size:16`, `maxzoom:0`, and zero offset. `sample.tif` is a 512×512 georeferenced COG generated by the source repository.

The core and CLI test suites use byte-identical copies under their respective `tests/data/` directories so packaged-crate tests never reach outside a package root. `scripts/verify-fixtures.sh` checks those copies against the canonical files and the three array/COG digests above; CI runs it explicitly.

Required checks:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features --locked
cargo +1.91.0 test --workspace --all-features --locked
cargo check -p zarra --no-default-features
npm --prefix example test
install_root="$(mktemp -d)"
cargo install --path crates/zarra-cli --root "$install_root"
"$install_root/bin/zarra" --help
rm -rf "${install_root:?}"
cargo package -p zarra --list --allow-dirty
cargo package -p zarra-cli --list --allow-dirty
```

The install and package-list commands verify local/path usage and file inclusion only. Actual `cargo package` and registry publication remain gated by `zarrs 0.23.15` availability as defined in §3.

A GDAL 3.13.1 container job runs the explicitly ignored integration tests:

```sh
cargo test -p zarra-cli --test gdal --all-features -- --ignored --nocapture
```

Those tests convert `sample.tif`, derive a three-band COG with repeated `-b 1` selections, reopen both outputs, verify 2D/3D layout and metadata, query known samples, and fetch native and overzoom PNGs. They also prove that a second conversion to the same output fails without changing the first output. The ordinary unit/integration suites do not require GDAL to be installed.

The final browser smoke uses `zarra serve testdata/refpy.zarr --port 8090`: load the printed loopback URL, confirm a nonblank 16×16-data map, use zoom/fit and keyboard navigation, resize to a mobile viewport, and confirm no failed local asset/tile requests. This visual check complements, rather than replaces, golden pixel and HTTP tests.
