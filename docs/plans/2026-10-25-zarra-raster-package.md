# Zarra Raster Package Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use subagent-driven development (recommended, one fresh subagent per task) or the executing-plans skill to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create a new `zarra` Rust workspace that ports Tilify Everything's GeoTIFF/COG → multiscale Zarr v3 creation, raster chunk/window/band querying, XYZ PNG rendering, and MapLibre visualization into an FGBA-style core/CLI/example package.

**Architecture:** Two publishable crates share one data path: `zarra` owns manifests, GDAL conversion, `zarrs`-backed local/HTTP reads, and PNG rendering; `zarra-cli` exposes that library through `convert`, `info`, `query`, `tile`, and a loopback `serve` command. A store is a directory/prefix with standard Zarr v3 arrays and a root-group `attributes.zarra` manifest, not an FGBA-style single file. The CLI embeds the viewer and range-capable demo server; `example/` supplies only npm launch/tests, so there is no second decoder or server implementation.

**Tech Stack:** Rust 2021, MSRV 1.91.0, `zarrs =0.23.14` (`filesystem,sharding,crc32c,zstd`, defaults off), optional `zarrs_http =0.3.1`, GDAL CLI 3.13.1, `serde`/`serde_json` 1, `thiserror` 2, `png =0.18.1`, `tempfile` 3, `clap` 4, MapLibre GL JS 4.7.1, dependency-free Node.js 20+ tests, Docker for GDAL integration.

**Spec:** `SPEC.md` (required: profile layout, public contracts, scope, fixtures, and acceptance criteria)

## Global Constraints

- Implement only in `/home/sheece/Desktop/work/zarra`; do not edit `/home/sheece/Desktop/work/tilifyeverything.com` or `/home/sheece/Desktop/work/fgba`.
- Follow `SPEC.md` exactly. Zarr is the upstream format; `zarra` is the package/profile name. Output is a `.zarr` directory/prefix, not a new single-file format.
- Workspace resolver 2, version 0.1.0, edition 2021, MSRV 1.91.0, license `Apache-2.0 OR MIT`; commit `Cargo.lock` and verify the complete graph on 1.91.0.
- Application code uses `#![forbid(unsafe_code)]`; workspace Clippy denies `unwrap_used`, `expect_used`, `panic`, `unimplemented`, `todo`, and `unreachable`. Tests may use ordinary `assert!`/`assert_eq!`, not `unwrap`/`expect`.
- Use `zarrs` for Zarr metadata/shard/codec behavior. Do not port `zarr_client.h`, `crc32c.h`, or `png_writer.h` into a parallel Rust codec stack.
- Disable unused `zarrs` defaults, including `ndarray`, Blosc, gzip, and transpose. The core's default feature set is empty; `http` is optional and enabled by the CLI.
- GDAL conversion uses `std::process::Command` with argv arrays and GDAL ≥3.13; never invoke a shell for source processing and never link libgdal. The compatibility target is `ghcr.io/osgeo/gdal:ubuntu-full-3.13.1`.
- Validate every trust boundary: root/level metadata, dimensions and allocation arithmetic, band selections, XYZ coordinates, URLs, HTTP routes, object paths, and output paths. Decode/transport errors stay typed errors and never silently become NaN.
- Preserve the canonical 2D/3D, tile-aligned, sharded layout and RG16/RGB8 semantics. Allow repeated band indices. Support exactly the eight dtypes in `SPEC.md`; reject mixed band dtypes/nodata and 64-bit integer/complex data.
- Build a local store in a sibling temporary directory and publish once with a directory rename. Never intentionally overwrite an existing store or tile. No distributed/remote multi-object atomicity claim and no concurrent-publisher coordination.
- Keep actual source visualization behavior: an ordinary MapLibre raster source at opacity 0.92. Do not add deck.gl, GPU colormap/hillshade controls, accounts, uploads, analytics, SQL, QGIS, signed S3, or a production service.
- Do not claim lossless scientific values in PNG, remote request/byte budgets, an LRU chunk cache, WASM support, arbitrary Zarr compatibility, or performance measurements not collected in this implementation.
- Keep the three fixture files' canonical bytes and recorded hashes. Packaged core/CLI tests use byte-identical copies under their own `tests/data/`; `scripts/verify-fixtures.sh` prevents drift.
- Every task ends with its narrow tests passing and a commit. A later task may extend CI only after the files that CI invokes exist.
- CI uses `permissions: contents: read` and full 40-character commit SHAs for every third-party action; mutable tags/branches are not allowed.
- Preserve explicit acceptance criteria from `SPEC.md`: real GDAL 2D/3D conversion, exact local/HTTP reference reads, golden pixels, deep-overzoom regression, safe range serving, real MapLibre data flow, and stable/MSRV/GDAL/Node checks.

---

## Session Research and Verified Facts

### Tilify Everything: the behavior to port

Paths below are relative to `/home/sheece/Desktop/work/tilifyeverything.com`.

| Area | Verified source evidence | Porting decision |
|---|---|---|
| Probe | `server/include/etl/raster_source.h:263-379` runs `gdalinfo -json -approx_stats`, a 3857 VRT warp, and a second `gdalinfo`; it extracts dtype, bands, nodata, palette, approximate ranges, extent, and native pixel size. | Keep the CLI probe. Reject mixed band dtypes and conflicting nodata because the root profile has shared values. Do not pass `-stats` with `-approx_stats`. |
| Pyramid | `raster_source.h:38-184,386-535` computes native maxzoom, snaps AOIs to WebMercatorQuad, chooses nodata, and runs warp/translate for maxzoom down to 0. | Port the pure math and argv construction with tests. Keep nearest for palette, bilinear otherwise, 256px inner chunks, ≤2048px shards, ZSTD level 5. |
| Layout | `raster_source.h:449-483` uses 2D `[H,W]` for one band and 3D `[bands,H,W]` otherwise. `server/include/etl/raster_pipeline.h:113-140` flattens `<L>/data/*` to `<L>/*` and discards coordinate/group artifacts. | Publish the same array layout. Retain one root group `zarr.json` with `attributes.zarra` so the new standalone package needs no PostgreSQL/JSON layer registry. |
| Query | `server/include/raster_array.h:48-217` provides level metadata, chunk enumeration, chunk/window/neighborhood/band reads. Only tile rendering consumes it in production; there is no public raster SQL or point-query route. | Expose those array reads through Rust and `zarra query`; do not invent a SQL adapter. Use `f64` output and propagate read errors instead of collapsing them to NaN/f32. |
| Shard reads | `server/include/zarr_client.h:150-327,379-583` parses metadata/indexes, performs range/suffix reads, verifies CRC32C, and decodes ZSTD. | Replace this custom stack with the already-maintained `zarrs` implementation, wrapped by strict canonical-profile validation. |
| Tile rendering | `server/routes/raster.h:70-278` resolves band selection, offsets, ancestor chunks, nodata, RG16/RGB8 packing; lines 304-399 implement the HTTP route. | Keep one framework-neutral `render_tile`. Use the `png` crate. One selected band → RG16; two/three → RGB8; only first selected band controls declared-nodata alpha. |
| Deep overzoom defect | `raster.h:348-357` caps `k` at 12 and then computes `L=z-k`, so `z>maxzoom+12` reads a nonexistent intermediate level. No existing test covers it. | Always use `level=maxzoom` when overzooming. Test `maxzoom`, `+1`, `+12`, `+13`, and zoom 22. |
| Visualization | `web/index.html:918-960` loads MapLibre 4.7.1 and adds an ordinary `{type:"raster"}` source/layer with tileSize 256 and opacity 0.92. | Reuse that actual behavior with the manifest's tile size. No deck.gl shader or terrain styling exists here to copy. |
| Unused metadata | `default_rescale`, `default_colormap`, `hillshade`, and `render_mode` are produced in `raster_source.h` but have no consumer elsewhere in the current repo. | Omit these disconnected fields from the v1 root manifest. |
| Dtype defect | `zarr_client.h` accepts uint64/int64 sizes but `DecodedChunk::value_at` has no cases and returns 0. | Reject both rather than preserving the mismatch. |
| Test assets | `server/tests/fixtures/refpy.zarr` is a `zarr-python 3.2.1` independent store; `sample.tif` is a 512×512 COG. Existing tests cover math/reference reads but not golden raster pixels or deep overzoom. | Reuse the fixtures and add the missing pixel/HTTP/GDAL/browser checks. |

The source's upload/S3/auth/registry/thread-pool machinery is service-specific and remains outside this package. In particular, `server/include/s3_client.h`, PostgreSQL/JSON registries, owner tokens, QGIS handoff, and upload lifecycle are not prerequisites for the local Rust library.

### FGBA: the package pattern to reuse

Paths below are relative to `/home/sheece/Desktop/work/fgba`.

- `Cargo.toml` and per-crate manifests establish versioned path dependencies, dual licensing, crate READMEs, committed lockfile, strict lints, and stable/MSRV CI. Keep those conventions with the new 1.91 dependency floor.
- `crates/fgba/src/lib.rs`, `error.rs`, and feature-gated `http.rs` demonstrate deliberate root exports, typed errors, and an optional native HTTP feature. Apply the pattern to keyed Zarr stores, not `Read + Seek` EOF files.
- `crates/fgba-cli/src/main.rs` demonstrates thin Clap dispatch, one command per module, exactly-one-of positional/`--url` source selection, and readable `ExitCode` errors.
- `crates/fgba-cli/src/serve.rs` provides a loopback, sequential, bounded-head, GET/HEAD range server with suffix/open/bounded ranges, 416, timeouts, and canonical-path escape rejection. Adapt those helpers to nested Zarr keys and add tile/viewer routes.
- `example/` proves the value of pinned local assets, licenses, no-npm-dependency Node tests, and a real end-to-end viewer. Do not copy its A5/deck.gl/FlatGeobuf implementation or create a second Zarr decoder.
- Per-crate fixture copies are intentional for packaged-crate tests. Keep that pattern; do not reference `../../testdata` from published crate tests.
- Do not copy FGBA's EOF footer, A5 bands, FlatGeobuf converters, DataFusion SQL, Arrow analytics, `Vec<u8>` whole-file builds, or single-file publication model.
- Avoid two reference-documentation mistakes: its `RELEASING.md` omits a required dependency publication, and its `serve` docs overstate viewer integration. Zarra has only core → CLI publication order, and `zarra serve` really will serve the viewer.
- `LICENSE-MIT` in the reference still has a placeholder copyright line. Zarra's file uses `Copyright (c) 2026 Zarra contributors` instead.

### Verified dependency/API facts

1. `cargo info zarrs@0.23.14` reports Rust 1.91 and default features `filesystem,ndarray,blosc,crc32c,gzip,sharding,transpose,zstd`. The selected reduced feature set is valid.
2. `zarrs` source `src/array/array_sync_readable.rs:30-53` verifies `Array::open(storage, path)`. `array_sharded_ext.rs` verifies `is_sharded`, `subchunk_shape`, and `subchunk_grid_shape`.
3. `array_sync_sharded_readable_ext.rs:51-69,184-302` verifies `ArrayShardedReadableExtCache::new`, `subchunk_byte_range`, `retrieve_subchunk_opt<T>`, and `retrieve_array_subset_sharded_opt<T>`. `Vec<T>` is a supported `FromArrayBytes` target; `ndarray` is unnecessary.
4. A compiled probe using the exact reduced features opened `refpy.zarr/0`, confirmed shape `[1,64,64]` and subchunks `[1,16,16]`, decoded `Vec<u16>`, and found `values[2*16+9] == 2009`. Out-of-grid `[0,4,4]` returned no byte range. A second metadata-only probe with no shard object also returned no byte range. Both probes exited 0.
5. `zarrs_http 0.3.1` declares Rust 1.82 and provides `HTTPStore::new` plus `set_batch_range_requests(false)`. Its source uses blocking `reqwest`, HEAD for size, and accepts full-body 200 responses in some range paths. It does not expose a total timeout. A separate compiled probe configured single-part ranges and exited 0 without network I/O; the resolved graph includes native TLS/OpenSSL, so the GDAL image installs its build prerequisites. These are documented limitations, not facts to hide.
6. `zarrs::group::Group::open(storage, "/")` and `Group::attributes()` are available and can read the root `attributes.zarra` manifest without a custom storage-key parser.
7. `png 0.18.1` provides `Encoder::new`, `set_color(ColorType::Rgba)`, `set_depth(BitDepth::Eight)`, `set_compression(Compression::Fast)`, `write_header`, and `write_image_data`. Encoding into `&mut Vec<u8>` avoids needing an `into_inner` API.
8. crates.io returned 404 `crate does not exist` for both `zarra` and `zarra-cli` on 2026-10-25, so both package names were available at planning time. Recheck immediately before publication.
9. MapLibre's package metadata confirms version 4.7.1 and BSD-3-Clause; `https://raw.githubusercontent.com/maplibre/maplibre-gl-js/v4.7.1/LICENSE.txt` exists and includes the required upstream notices.

### Planning environment and fixture evidence

- Planning host: `/home/sheece/.cargo/bin/cargo` 1.99.1 and Node 24.19.0 are installed. `cargo` is not currently on the default shell PATH, so local execution may start with `export PATH="$HOME/.cargo/bin:$PATH"`.
- GDAL executables are not installed on the host; `/usr/bin/docker` and `/usr/bin/curl` are available. `scripts/test-gdal.sh` therefore builds a reproducible GDAL/Rust test image instead of silently skipping creation tests.
- Canonical fixture hashes, measured locally:
  - `refpy.zarr/0/zarr.json`: `a609e165541e8c062b7662564e6d64c4e5c6e94ecba914dc9202d041a2f66104`
  - `refpy.zarr/0/c/0/0/0`: `8e390da5ecd278c318d586f112d1484e7836047e22b0d487fbef32780b071e17`
  - `sample.tif`: `9b4604471f6a2bb5674ba990c28c3f51c26d1c201d849581225c968d25dd2664`
- `sample.tif` is generated by `scripts/make_fixtures.sh` in the source repo with EPSG:32630 georeferencing. It is small enough for routine GDAL integration tests.

### Data flow

```text
                         ┌────────────────────────────┐
 GeoTIFF/COG → GDAL CLI  │ create/{webmercator,gdal}  │
                         └─────────────┬──────────────┘
                                       │ sibling temp build + one rename
                                       ▼
                    root zarr.json + 0..maxzoom canonical arrays
                                       │
                    ┌──────────────────┴───────────────────┐
                    ▼                                      ▼
          FilesystemStore / HTTPStore              `zarra convert/info`
                    │                                      │
          strict manifest/level validation                 │
                    │                                      │
          ArrayShardedReadableExtCache                     │
                    │                                      │
       ┌────────────┴─────────────┐                        │
       ▼                          ▼                        ▼
 chunk/window/band query    render_tile → RGBA8 PNG    `zarra query/tile`
       │                          │                        │
       ▼                          ▼                        ▼
 `zarra query` JSON        `zarra tile` / serve route   loopback serve → MapLibre
```

### Support/scope matrix

| Capability | Included in v1 | Deferred and why |
|---|---|---|
| Store | Local directory; optional keyed HTTP | Signed S3 and remote publication need service credentials/lifecycle outside this package |
| Creation | GeoTIFF/COG through GDAL 3.13+ | Native Rust reprojection/resampling is a separate geospatial engine project |
| Arrays | Canonical Zarr v3, 2D/3D, default `/` keys, LE bytes, optional inner ZSTD, CRC32C sharding | Arbitrary codecs/dtypes/layouts are not source-compatible requirements |
| Query | Level/chunk/enumeration/window/neighborhood/band reads | SQL has no source implementation; no analytics sidecar or Arrow schema is justified |
| Rendering | Native/overzoom XYZ, RG16/RGB8, nodata, 200 PNG/204 empty | Terrain color/hillshade/GPU restyle has no current source implementation |
| Viewer | MapLibre raster, extent fit, navigation, metadata/XYZ telemetry | Object-storage browser and direct JS decoder would duplicate the Rust/server path |
| Package | Core + CLI + embedded viewer + npm launcher/tests + docs/CI | No SQL crate, xtask, WASM binding, Pages site, or production server in the initial package |

### Risks and mitigations

| Risk | Mitigation |
|---|---|
| `zarrs` API/MSRV drift | Exact direct pins, committed lockfile, compiled reference probe, dedicated 1.91.0 locked job |
| GDAL missing or advertises misleading sharding support | Use GDAL 3.13.1 container and prove actual `sharding_indexed` output through conversion/reopen tests; do not grep only `gdalinfo` |
| Native C/TLS build prerequisites differ by host | Compile the optional HTTP feature during planning and install `build-essential`, `pkg-config`, and `libssl-dev` in the GDAL test image |
| Hostile metadata triggers large allocation/incorrect addressing | Strict canonical JSON checks, checked arithmetic, 4,194,304-value chunk and 16,777,216-value query caps before reads/allocation |
| Source deep-overzoom bug is copied | Shared ancestor resolver plus `+13` and zoom-22 golden route tests |
| Sparse data is confused with errors | `subchunk_byte_range` determines `None`; decode/transport failures remain `Err`; window NaN is reserved for outside/sparse |
| Multi-band semantics are overstated | Validate shared dtype/nodata, allow at most three selected bands, document first-band alpha and no fourth/alpha-band behavior |
| HTTP store hangs, downloads a full body, or follows weaker redirects | Keep HTTP optional for trusted read-only stores; validate initial scheme/host/userinfo/query/fragment, disable batched ranges, and publish no timeout/request/downgrade guarantee |
| Existing output is damaged | Require absent output, same-parent temp build, no-force conversion, create-new tile output, and unchanged-tree assertion after a rejected second conversion |
| Packaged tests lose fixtures or fixtures drift | Keep per-crate byte-identical copies and run `scripts/verify-fixtures.sh` in CI |
| Viewer looks connected but bypasses real data | Node tests import the same `geo.js` used by the browser, assert the real tile template, fetch actual PNGs, and run a final browser smoke |
| Documentation copies FGBA-specific claims | Regenerate commands/support/release order; no FGB/A5/SQL/WASM/performance text carried forward |

---

## File Structure

| Area | Files | Responsibility |
|---|---|---|
| Workspace | `Cargo.toml`, `Cargo.lock`, `.gitignore`, `.github/workflows/ci.yml` | Shared metadata/lints, locked dependencies, ignored artifacts, stable/MSRV/GDAL/Node CI |
| Profile | `SPEC.md` | Existing authoritative profile and acceptance contract from this planning session |
| Core manifest | `crates/zarra/src/{lib,error,manifest,level}.rs` | Public exports, typed errors, root manifest/DType/nodata, strict level validation |
| Core creation | `crates/zarra/src/create/{mod,webmercator,gdal}.rs` | Pure tile/nodata planning, GDAL probe/argv, temp build/flatten/publication |
| Core read/query | `crates/zarra/src/{store,query}.rs` | Local/HTTP backends, level/cache ownership, exact chunk/window/band reads |
| Core rendering | `crates/zarra/src/render.rs` | XYZ/ancestor/band resolution, golden-testable packing, PNG output |
| Core tests | `crates/zarra/tests/{manifest,webmercator,refpy,http,render}.rs` | Profile, math, local/HTTP reference, rendering regressions |
| Core fixtures | `crates/zarra/tests/data/refpy.zarr/**` | Packaged independent reference store |
| CLI dispatch | `crates/zarra-cli/src/{main,source,convert,info,query,tile}.rs` | Clap commands, source validation, JSON/text/binary output |
| CLI server | `crates/zarra-cli/src/{serve,range,viewer}.rs` | Loopback routing, safe nested range responses, embedded assets |
| CLI tests | `crates/zarra-cli/tests/{cli,serve,gdal}.rs` | Real binary contracts, server/PNG/range behavior, ignored GDAL conversions |
| CLI fixtures | `crates/zarra-cli/tests/data/{refpy.zarr/**,sample.tif}` | Packaged server/CLI/GDAL inputs |
| Browser assets | `crates/zarra-cli/assets/{index.html,app.js,geo.js,styles.css,favicon.svg}`, `assets/vendor/**` | Actual MapLibre viewer and shared testable URL/source logic |
| Example | `example/{package.json,README.md,viewer.test.mjs}` | npm launcher and dependency-free integration tests; no second server |
| Verification | `scripts/{verify-fixtures.sh,test-gdal.sh,Dockerfile.gdal}` | Fixture drift gate and reproducible GDAL 1.91/3.13.1 test environment |
| Package docs | `README.md`, per-crate READMEs, `ATTRIBUTION.md`, `CHANGELOG.md`, `RELEASING.md`, `LICENSE-*` | Accurate scope/quickstart/provenance/release process |

No `zarra-sql`, `xtask`, `docs/reference`, WASM directory, production deployment files, or second JavaScript server is created.

---

## Reference Implementations

### A. Compiled `zarrs` compatibility probe

This is the final probe that passed against the source fixture during planning. Task 3 turns its assertions into permanent tests rather than asking the executor to rediscover the API.

```rust
use std::sync::Arc;
use zarrs::array::{
    Array, ArrayShardedExt, ArrayShardedReadableExt, ArrayShardedReadableExtCache, CodecOptions,
};
use zarrs::filesystem::FilesystemStore;
use zarrs::storage::ReadableStorage;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = "crates/zarra/tests/data/refpy.zarr";
    let store: ReadableStorage = Arc::new(FilesystemStore::new(root)?);
    let array = Array::open(store, "/0")?;
    assert_eq!(array.shape(), &[1, 64, 64]);
    assert!(array.is_sharded());
    let subchunk = array.subchunk_shape().ok_or("missing subchunk shape")?;
    assert_eq!(subchunk.iter().map(|v| v.get()).collect::<Vec<_>>(), [1, 16, 16]);

    let cache = ArrayShardedReadableExtCache::new(&array);
    let values: Vec<u16> =
        array.retrieve_subchunk_opt(&cache, &[0, 0, 0], &CodecOptions::default())?;
    assert_eq!(values.len(), 16 * 16);
    assert_eq!(values[2 * 16 + 9], 2009);
    assert!(array.subchunk_byte_range(&cache, &[0, 0, 0])?.is_some());
    assert!(array.subchunk_byte_range(&cache, &[0, 4, 4])?.is_none());
    Ok(())
}
```

The sparse variant uses the same open/cache calls against a temporary fixture containing `0/zarr.json` but no `0/c/0/0/0`; `subchunk_byte_range(&cache, &[0,0,0])` returns `Ok(None)`.

### B. Canonical root fixture manifest

Write this as `testdata/refpy.zarr/zarr.json`, then copy it byte-for-byte to both crate fixture stores. It adds no changes to the independent array files.

```json
{
  "zarr_format": 3,
  "node_type": "group",
  "attributes": {
    "zarra": {
      "profile_version": 1,
      "crs": 3857,
      "tile_size": 16,
      "dtype": "uint16",
      "bands": 1,
      "nodata": null,
      "minzoom": 0,
      "maxzoom": 0,
      "levels": [{ "z": 0, "xoff": 0, "yoff": 0 }],
      "tile_encoding": {
        "channels": "rg16",
        "encoded_min": 0.0,
        "encoded_max": 63063.0
      },
      "per_band_range": [[0.0, 63063.0]],
      "extent": [-180.0, -85.0, 180.0, 85.0]
    }
  }
}
```

### C. Correct ancestor selection

Use one shared resolver in `render.rs`; do not cap `k` and accidentally select an intermediate level.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Ancestor {
    level: u8,
    ox: u32,
    oy: u32,
    winx: u32,
    winy: u32,
    factor: u32,
}

fn ancestor(tile: TileCoord, maxzoom: u8) -> Ancestor {
    let k = tile.z.saturating_sub(maxzoom);
    let factor = 1u32 << k;
    Ancestor {
        level: tile.z - k,
        ox: tile.x >> k,
        oy: tile.y >> k,
        winx: tile.x & (factor - 1),
        winy: tile.y & (factor - 1),
        factor,
    }
}

#[test]
fn overzoom_always_uses_maxzoom() -> Result<(), Box<dyn std::error::Error>> {
    for delta in [1, 12, 13, 22] {
        let a = ancestor(TileCoord { z: delta, x: 0, y: 0 }, 0);
        assert_eq!(a.level, 0);
        assert_eq!(a.factor, 1u32 << delta);
    }
    Ok(())
}
```

`TileCoord::validate` runs first, so `z <= 22` and the shift is defined.

### D. PNG encoding without a custom container writer

```rust
fn encode_rgba(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| Error::InvalidInput("PNG dimensions overflow".into()))?;
    if rgba.len() != expected {
        return Err(Error::InvalidInput("RGBA buffer length mismatch".into()));
    }
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder
            .write_header()
            .map_err(|e| Error::Png(e.to_string()))?;
        writer
            .write_image_data(rgba)
            .map_err(|e| Error::Png(e.to_string()))?;
    }
    Ok(out)
}
```

### E. Reproducible GDAL test container

`scripts/Dockerfile.gdal`:

```dockerfile
FROM rust:1.91.0 AS rust
FROM ghcr.io/osgeo/gdal:ubuntu-full-3.13.1
RUN apt-get update -qq \
 && apt-get install -y -qq --no-install-recommends \
    build-essential pkg-config libssl-dev ca-certificates \
 && rm -rf /var/lib/apt/lists/*
COPY --from=rust /usr/local/cargo /usr/local/cargo
COPY --from=rust /usr/local/rustup /usr/local/rustup
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:${PATH}
WORKDIR /src
CMD ["cargo", "test", "-p", "zarra-cli", "--test", "gdal", "--all-features", "--", "--ignored", "--nocapture"]
```

`scripts/test-gdal.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
image=zarra-gdal-test:3.13.1
docker build -f "$root/scripts/Dockerfile.gdal" -t "$image" "$root/scripts"
docker run --rm \
  --user "$(id -u):$(id -g)" \
  -e HOME=/tmp -e CARGO_HOME=/tmp/.cargo \
  -v "$root:/src" -w /src "$image"
```

The mount keeps build artifacts owned by the host user; `/tmp/.cargo` is the container-local registry cache. No production image or deployment claim is added.

---

## Task 1: Workspace, manifest, fixtures, and baseline CI

**Files:**
- Create: `Cargo.toml`, `.gitignore`, `crates/zarra/Cargo.toml`, `crates/zarra-cli/Cargo.toml`
- Create: `crates/zarra/src/{lib,error,manifest}.rs`, `crates/zarra/tests/manifest.rs`
- Create: `crates/zarra-cli/src/main.rs` (minimal working Clap `--help` entry point, extended with real subcommands in Task 2)
- Create: `testdata/refpy.zarr/**`, `testdata/sample.tif`
- Create: `crates/zarra/tests/data/refpy.zarr/**`
- Create: `crates/zarra-cli/tests/data/{refpy.zarr/**,sample.tif}`
- Create: `scripts/verify-fixtures.sh`, `.github/workflows/ci.yml`
- Existing: `SPEC.md`

**Interfaces:**
- Produces: `DType`, `Channels`, `Nodata`, `LevelOffset`, `TileEncoding`, `RasterManifest`, `Error`, and `Result<T>` exactly as `SPEC.md` defines.
- `DType::from_gdal(&str) -> Option<DType>` and `DType::as_str(self) -> &'static str`.
- `RasterManifest::from_attributes(&serde_json::Map<String, Value>) -> Result<RasterManifest>`.
- `RasterManifest::to_value() -> Result<Value>` returns the manifest object, not the outer `zarra` wrapper.
- `RasterManifest::validate() -> Result<()>` and `level(z: u8) -> Option<LevelOffset>`.
- CLI starts with no public commands beyond a real `zarra --help`; Task 2 adds conversion and Task 3 adds query/info.

- [ ] **Step 1: Initialize the target repository and write the workspace manifests**

Run `git init` in `/home/sheece/Desktop/work/zarra`. Copy the root workspace conventions from FGBA but use only members `crates/zarra` and `crates/zarra-cli`, MSRV 1.91, and repository `https://github.com/afterrealism/zarra`.

Core dependencies are `serde` with `derive`, `serde_json`, `thiserror`, `zarrs` with exact reduced features from the header, optional `zarrs_http`/`url`, `png`, and `tempfile`. Declare `default = []` and `http = ["dep:zarrs_http", "dep:url"]`. Add docs.rs `features = ["http"]` and package include `src/**`, `tests/**`, `Cargo.toml`, `README.md`. The CLI has a binary named `zarra`, `clap` with `derive`, `serde`/`serde_json`, and `zarra = { version = "0.1.0", path = "../zarra", features = ["http"] }`; its package include is `src/**`, `assets/**`, `tests/**`, `Cargo.toml`, `README.md`.

Write `.gitignore`:

```gitignore
/target
*.tmp
/example/data/
.DS_Store
```

Write `src/lib.rs` with the lint policy and only `pub mod error; pub mod manifest;` plus deliberate root exports. Write the error variants `Io`, `Gdal`, `Format`, `Unsupported`, `Zarr`, `Http`, `Png`, and `InvalidInput`, using `thiserror` and `Result<T>`. Do not add modules before the tasks that implement them.

- [ ] **Step 2: Copy the independent fixtures and add the root manifests**

Create all three `refpy.zarr` copies and two `sample.tif` copies:

```bash
mkdir -p testdata crates/zarra/tests/data crates/zarra-cli/tests/data
for dst in testdata crates/zarra/tests/data crates/zarra-cli/tests/data; do
  cp -a /home/sheece/Desktop/work/tilifyeverything.com/server/tests/fixtures/refpy.zarr "$dst/"
done
for dst in testdata crates/zarra-cli/tests/data; do
  cp /home/sheece/Desktop/work/tilifyeverything.com/server/tests/fixtures/sample.tif "$dst/sample.tif"
done
```

Write Reference Implementation B as `testdata/refpy.zarr/zarr.json`, then copy that root file to both crate fixture roots. This is an additive root-group file; never rewrite the two source array files.

- [ ] **Step 3: Write failing manifest and fixture tests**

In `crates/zarra/tests/manifest.rs`, construct `attributes` from Reference Implementation B and cover roundtrip plus hostile values:

```rust
use zarra::{DType, RasterManifest};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn attributes() -> serde_json::Map<String, serde_json::Value> {
    let root: serde_json::Value = serde_json::from_str(include_str!(
        "data/refpy.zarr/zarr.json"
    ))
    .unwrap_or(serde_json::Value::Null);
    root["attributes"].as_object().cloned().unwrap_or_default()
}

#[test]
fn manifest_roundtrip_and_lookup() -> TestResult {
    let manifest = RasterManifest::from_attributes(&attributes())?;
    assert_eq!(manifest.dtype, DType::Uint16);
    assert_eq!(manifest.tile_size, 16);
    assert_eq!(manifest.level(0).map(|l| (l.xoff, l.yoff)), Some((0, 0)));
    assert!(manifest.level(1).is_none());
    let encoded = manifest.to_value()?;
    let reparsed: RasterManifest = serde_json::from_value(encoded)?;
    reparsed.validate()?;
    assert_eq!(reparsed, manifest);
    Ok(())
}

#[test]
fn manifest_rejects_missing_level_and_u64() -> TestResult {
    let mut attrs = attributes();
    attrs["zarra"]["maxzoom"] = serde_json::json!(1);
    assert!(RasterManifest::from_attributes(&attrs).is_err());
    let mut attrs = attributes();
    attrs["zarra"]["dtype"] = serde_json::json!("uint64");
    assert!(RasterManifest::from_attributes(&attrs).is_err());
    Ok(())
}
```

The `unwrap_or` calls are intentional non-panicking test helpers and comply with the Clippy ban. Add tests for non-finite ranges, NaN nodata on integers, incomplete level sequences, invalid band counts, invalid CRS/profile version, an offset outside its zoom grid, an encoded range that differs from band 0, and an out-of-WGS84 extent.

Write `scripts/verify-fixtures.sh`:

```bash
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
```

Make it executable and run it. Expected: all comparisons/hashes pass.

- [ ] **Step 4: Run the tests to establish the expected failure**

Run `cargo test -p zarra --test manifest`. Expected: compilation FAIL because `manifest.rs` does not yet define the public types/methods. Do not weaken or delete the tests.

- [ ] **Step 5: Implement manifest serialization and validation**

Derive `Debug`, `Clone`, `PartialEq`, `Serialize`, and `Deserialize` where appropriate; also derive `Copy` for `DType`, `Channels`, `Nodata`, `LevelOffset`, and `TileEncoding`. Use lowercase serde names for `DType`/`Channels`. Implement custom `Nodata` serde so `Number` is a JSON number and `NaN` is the string `"NaN"`; `Option<Nodata>` handles `null`.

`from_attributes` must require an own `zarra` member, deserialize it, and call `validate`. `validate` implements every rule in `SPEC.md §4`, including complete 0..=maxzoom coverage, finite ordered ranges, matching channel/band counts, and floating-point-only NaN. `level` returns a copied offset by value so query/render code does not hold borrows into the manifest.

- [ ] **Step 6: Add the baseline workflow and make the tests pass**

Create CI with a stable job that runs, in order:

```bash
bash scripts/verify-fixtures.sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features --locked
cargo check -p zarra --no-default-features
```

Add an MSRV job using `dtolnay/rust-toolchain@b725eda525362e7cf34af9a7243cac1e3f01a7ec` with `toolchain: 1.91.0`, then `cargo test --workspace --all-features --locked`. Use `actions/checkout@11d5960a326750d5838078e36cf38b85af677262` in every job. Do not add Node/GDAL steps until their files exist.

Run the same relevant local commands plus the manifest test. Expected: all pass; `Cargo.lock` is generated and committed.

- [ ] **Step 7: Commit the independently reviewable scaffold**

```bash
git add .
git commit -m "feat: add zarra workspace and raster manifest"
```

---

## Task 2: GDAL conversion and `zarra convert`

**Files:**
- Create: `crates/zarra/src/create/{mod,webmercator,gdal}.rs`, `crates/zarra/src/level.rs`
- Create: `crates/zarra/tests/{webmercator,level}.rs`
- Create: `crates/zarra-cli/src/convert.rs`
- Modify: `crates/zarra/src/lib.rs`, `crates/zarra-cli/src/main.rs`
- Create: `crates/zarra-cli/tests/gdal.rs`
- Create: `scripts/{Dockerfile.gdal,test-gdal.sh}`
- Modify: `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: Task 1 manifest/DType/nodata and errors.
- Produces: `pub fn convert(input: &Path, output: &Path) -> Result<BuildReport>`, plus `LevelMeta` and the shared canonical JSON validator in `level.rs`.
- `BuildReport { output: PathBuf, manifest: RasterManifest, levels: usize, files: u64 }`.
- Public pure helpers `webmerc_res`, `webmerc_tilespan`, `native_maxzoom`, `tile_range_for`, `pick_dst_nodata`, and `level_shapes` with integration tests.
- Crate-private `SourceInfo::from_gdal_json(original: &Value, warped_vrt: &Value) -> Result<SourceInfo>` keeps GDAL JSON interpretation testable with inline tests and no public GDAL-type exposure.
- CLI `zarra convert <INPUT.tif> -o <OUTPUT.zarr>` accepts only local input/output paths.

- [ ] **Step 1: Port the pure math tests before implementation**

Translate every assertion from `server/tests/test_raster_source.cpp` into `crates/zarra/tests/webmercator.rs`: exact native zoom at levels 0/3/8/12/18/22, dtype mapping, world/NW/centered tile ranges, aligned coverage at levels 5/10/14, and every nodata case. Add shape tests:

```rust
use zarra::create::webmercator::level_shapes;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn one_band_drops_band_axis_but_multiband_keeps_it() -> TestResult {
    assert_eq!(level_shapes(1, 3, 2)?, ("512,768".into(), "256,256".into()));
    assert_eq!(level_shapes(3, 3, 2)?, ("3,512,768".into(), "3,256,256".into()));
    assert_eq!(level_shapes(4, 9, 1)?, ("4,256,2048".into(), "4,256,256".into()));
    Ok(())
}
```

Run `cargo test -p zarra --test webmercator`. Expected: FAIL because the module/functions do not exist.

Also create `crates/zarra/tests/level.rs` from the fixture's `0/zarr.json` and Reference Implementation B. Test a valid end-index/ZSTD level, an accepted start-index level, accepted bytes-only inner codecs, and rejection of big-endian bytes, non-default chunk keys, Blosc/other codecs, non-square or nonmultiple chunks, band-shape mismatches, uint64/int64, storage transformers, and metadata exceeding the chunk-value cap. Assert the error category/message for the cap rather than merely accepting any error.

Run `cargo test -p zarra --test level`. Expected: FAIL because `LevelMeta::from_json` does not exist.

- [ ] **Step 2: Implement pure WebMercator/nodata/shape planning**

Port `WORLD`, `webmerc_res`, `webmerc_tilespan`, `native_maxzoom`, `TileRange`, `tile_range_for`, and `pick_dst_nodata` from `raster_source.h:38-184` using checked zoom/shape arithmetic. `level_shapes` returns `(BLOCKSIZE, SHARD_CHUNK_SHAPE)` for touched tile columns/rows; spatial outer edge is `min(2048, tiles * 256)`, inner is 256. Preserve the 2D-vs-3D band-axis rule.

Run the test again. Expected: all pure tests pass.

- [ ] **Step 3: Write failing GDAL orchestration tests**

In `crates/zarra-cli/tests/gdal.rs`, write `#[ignore = "requires GDAL >= 3.13"]` integration tests that launch `env!("CARGO_BIN_EXE_zarra")` and:

1. Convert `tests/data/sample.tif` into a fresh temp `one.zarr`.
2. Run `gdal_translate -of COG -b 1 -b 1 -b 1 sample.tif rgb.tif`, then convert `rgb.tif` into `three.zarr`.
3. In this task, inspect root/level JSON directly and assert 2D `[H,W]` vs 3D `[3,H,W]`, `sharding_indexed`, 256 inner shape, and the root manifest. Task 3 upgrades the same test to reopen through `RasterStore`.
4. Assert no `_warp_`, `/data/`, X/Y coordinate arrays, or temporary build paths were published.
5. Snapshot all output files into a `BTreeMap<PathBuf, Vec<u8>>`, run the same conversion again, assert failure, and assert the snapshot is unchanged.

Mark the root/level JSON helper `fn read_json(path: &Path) -> Result<Value, Box<dyn std::error::Error>>` without `unwrap`/`expect`. The test must fail at this point because `convert` is unimplemented.

Also add inline `create/gdal.rs` tests for `SourceInfo::from_gdal_json`: valid one/three-band metadata, mixed dtypes, conflicting or mixed-present/absent nodata, NaN nodata, missing CRS/GCP, missing bands, palette selection, WGS84 extent recursion, invalid geoTransform/cornerCoordinates, and non-finite/native-zero pixel size. These parser tests must fail before the pure parser exists and must not invoke GDAL.

- [ ] **Step 4: Implement GDAL probe and per-level emission**

In `create/gdal.rs`, implement a `Gdal` runner around `Command::output`: resolve `GDAL_BIN_DIR`/PATH, capture status/stdout/stderr, and return bounded (last 8 KiB) stderr in `Error::Gdal`. Never build a shell command string.

Implement `SourceInfo::from_gdal_json` per `SPEC.md §5.2` and make its inline tests pass: original JSON + VRT JSON, all-band dtype/nodata agreement, palette, approximate ranges, extent, georeference, 3857 footprint, and native pixel size. Validate finite values and nonempty bands; treat two NaN declarations as agreeing and reject other non-finite nodata values. `Gdal::probe_source` only runs the tools and delegates parsing to this pure function.

Implement level emission per `SPEC.md §5.4`: `gdalwarp` exact aligned extent/resolution/nodata/resampling, then `gdal_translate` with `level_shapes`. Use `format!("{value:.10}")` for GDAL floating arguments, matching the source's precision.

- [ ] **Step 5: Implement temporary build, flattening, validation, and publication**

First implement `LevelMeta::from_json` as a public associated function with signature `fn from_json(value: &Value, z: u8, manifest: &RasterManifest) -> Result<LevelMeta>` in `level.rs`, covering every canonical rule and allocation guard in `SPEC.md §6` so the external `tests/level.rs` suite exercises the same production validator. Then, in `create/mod.rs`:

1. Canonicalize the input and existing output parent; require nonempty regular input and an absent output path.
2. Create `tempfile::Builder::tempdir_in(parent)` and child `work`/`store` paths.
3. Probe and emit every level into `work/<L>`.
4. Create `store` once. Move `work/<L>/data` to `store/<L>` with `fs::rename`; this publishes only pixel-array metadata/chunks and leaves group/X/Y/warp artifacts in `work`.
5. Read `work/zarr.json`, require a v3 group, insert `attributes.zarra = manifest.to_value()?`, and write `store/zarr.json` with `create_new(true)`.
6. Validate the manifest and call `LevelMeta::from_json` on every published level's array metadata, using the shared validator in `level.rs`; count regular files recursively using `std::fs::read_dir`, recheck output absence, and rename `store` to the requested output.
7. Let the temporary parent clean failures/intermediates. Return `BuildReport`; do not add a force flag or upload code.

Run `cargo test -p zarra --test level`. Expected: all canonical acceptance/rejection and allocation-cap tests pass.

Add `convert` to `src/lib.rs` exports. Wire `zarra convert` through Clap and print output path, levels, files, dtype, bands, and maxzoom. `convert` always requires a positional local input; it has no `--url` mode.

- [ ] **Step 6: Run default tests and the real GDAL suite**

Make `scripts/test-gdal.sh` executable using Reference Implementation E. Run:

```bash
cargo test --workspace --all-features --locked
bash scripts/test-gdal.sh
```

Expected: default tests pass without host GDAL; the container builds the store successfully, 2D/3D and cleanup assertions pass, and repeat conversion fails without changing the first output.

Extend CI with a `gdal` job that checks out the repo and runs `bash scripts/test-gdal.sh`. This uses the same tested container locally and in CI.

- [ ] **Step 7: Commit conversion**

```bash
git add .
git commit -m "feat: convert rasters to canonical multiscale zarr"
```

---

## Task 3: Local/HTTP querying plus `info` and `query`

**Files:**
- Create: `crates/zarra/src/{store,query}.rs`
- Modify: `crates/zarra/src/level.rs`
- Create: `crates/zarra/tests/{refpy,http}.rs`
- Create: `crates/zarra-cli/src/{source,info,query}.rs`
- Modify: `crates/zarra/src/lib.rs`, `crates/zarra-cli/src/main.rs`
- Modify: `crates/zarra-cli/tests/gdal.rs`
- Create: `crates/zarra-cli/tests/cli.rs`

**Interfaces:**
- Consumes: Task 1 manifest and Task 2 converted stores.
- Produces: `LevelMeta`, `ChunkRef`, `Chunk`, `Window`, `RasterStore`, and all public signatures in `SPEC.md §7`.
- `parse_band_list(text: &str) -> Result<Vec<usize>>` is the shared syntax parser for CLI query/tile and the HTTP route: comma-separated nonnegative decimal indices with optional `b`/`B` prefixes; reject empty/malformed tokens without applying defaults.
- `RasterStore::open` and feature-gated `open_url` return an owned store with `ReadableStorage`, manifest, and per-level `Array`/`ArrayShardedReadableExtCache` state.
- CLI `zarra info` and `zarra query` use the same `Source::open() -> Result<RasterStore>` as later `tile`.

- [ ] **Step 1: Turn the compiled probe into failing permanent reference tests**

Write `crates/zarra/tests/refpy.rs` using `concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/refpy.zarr")`. Cover:

- `manifest()` and `level_meta(0)`: one band, 64×64, inner 16, `Uint16`.
- `enumerate_chunks(0)`: 16 coordinates.
- Every chunk `(row,col)` in 0..4: `value(0,r,c) == (global_y*1000+global_x) & 0xffff`.
- `read_chunk(0,9,9)` returns `Ok(None)`.
- Window 4,4/8×8 inside one chunk and 12,12/8×8 crossing four chunks, both exact.
- Neighborhood at 0,0/radius 1 has NaN at outside cells and exact interior values.
- `read_bands(...,&[0,0])` returns two identical channels in requested order.
- Out-of-range band, zero dimensions, oversized radius/window, malformed level metadata, and a v2 root group are errors.
- Copy only `0/zarr.json` into a temp store with the root manifest and verify missing shard reads are sparse, not transport errors.

Run `cargo test -p zarra --test refpy`. Expected: FAIL because store/query types do not exist.

- [ ] **Step 2: Implement strict level validation and local opening**

In `level.rs`, define `pub(crate) type DynArray = Array<dyn ReadableStorageTraits>` and add `LevelMeta::from_array(&DynArray, z, &RasterManifest)`: serialize `array.metadata()` to `serde_json::Value` once per opened level and delegate to Task 2's `LevelMeta::from_json`. This preserves one canonical validator for version/node/grid/key encoding/dimensions/dtype/sharding/codec order/index/end-start/band axes/square inner/tile size/storage transformers. Derive `LevelMeta` only after checks and allocation caps pass.

In `store.rs`, import `DynArray` from `level.rs` and define a private `Level` holding `DynArray`, `LevelMeta`, and `ArrayShardedReadableExtCache`. `RasterStore` owns `ReadableStorage`, `RasterManifest`, and `HashMap<u8, Level>`.

For local open, canonicalize the directory, create `FilesystemStore`, open `Group::open(storage.clone(), "/")`, require Zarr v3 root-group metadata, and parse `RasterManifest::from_attributes(group.attributes())`. Then call `level_meta(maxzoom)` to fail early on mismatches. Cache levels on first use. Before each chunk read, sum `ArrayShardedReadableExtCache::len()` across levels and clear all level caches when the store-wide total has reached 1024; do not allow a separate 1024-entry budget per level or advertise a compressed-chunk LRU.

- [ ] **Step 3: Implement chunk/window/band reads with typed sparse/error semantics**

In `query.rs`, implement `read_chunk` by constructing `[row,col]` or `[0,row,col]` subchunk indices, calling `subchunk_byte_range`, returning `Ok(None)` when absent, and decoding `Vec<T>` with a small dtype dispatch macro. Convert into band-planar `Vec<f64>` and verify exact expected length. `Chunk::value` returns `None` for any out-of-range coordinate instead of indexing unchecked.

Implement `read_window` with a NaN-filled, band-interleaved output and clamped iteration over intersecting inner chunks. Sparse chunks remain NaN; any actual error is returned. Reorder zarrs' band-planar values into `SPEC.md`'s output layout. `read_bands` shares the same internal implementation and validates all indices before reading; repeats are allowed. `read_neighborhood` uses checked `2r+1` and coordinate arithmetic. `enumerate_chunks` checks grid/cap before allocation.

Run the reference test. Expected: all exact values, NaN padding, sparse behavior, and error cases pass.

- [ ] **Step 4: Add optional HTTP storage and URL tests**

Implement `open_url` with `url::Url`: reject userinfo/query/fragment and unsupported schemes; permit HTTPS everywhere and HTTP only for `localhost`, an IPv4 address whose first octet is 127, or IPv6 loopback. Strip only trailing `/` from a validated base before `HTTPStore::new`, then call `set_batch_range_requests(false)`. Delegate to the same private open-with-storage helper as local opening so root-v3, manifest, and maxzoom validation are not duplicated.

In `crates/zarra/tests/http.rs`, write a small sequential loopback server adapted from FGBA's `crates/fgba/tests/http.rs`, extended to nested paths, HEAD, suffix/bounded ranges, and `Content-Range`. Serve the CLI/core fixture directory and test `open_url("http://127.0.0.1:<port>")` + `read_chunk(0,0,0)`. Add pre-network scheme assertions for `ftp`, `file`, non-loopback HTTP, userinfo, query, and fragment. Do not assert a request budget or 200-only rejection that `zarrs_http` does not provide.

- [ ] **Step 5: Implement `info`, streaming query JSON, and real CLI tests**

`Source` resolves `(Some(path), None)` and `(None, Some(url))`, rejecting both/neither. `Source::open` delegates to local/`open_url`.

`info` prints store label, CRS, dtype, bands, tile size, min/max zoom, encoding, extent, and each available level's shape/inner/offset.

`query` calls `read_bands` when `--bands` is present and `read_window` otherwise. Use `parse_band_list` for the option syntax, then let `read_bands` enforce band existence and the query allocation cap; do not impose the tile-only three-channel limit on general array queries. Implement a `JsonWindow<'a>` serializer with `SerializeStruct` and a `SerializeSeq` wrapper over `&[f64]`; emit finite numbers directly and non-finite values as `"NaN"`, `"Infinity"`, or `"-Infinity"`. Stream through `serde_json::to_writer_pretty(stdout, ...)` rather than building one JSON `Value` per sample. Add an inline serializer test with `[NaN, +∞, -∞, 1.5]` and assert all four exact JSON representations.

In `crates/zarra-cli/tests/cli.rs`, launch the real binary. Assert `info` contains `uint16` and `16`; query 12,12/8×8 returns 64 values beginning at 12012; query -1,-1/2×2 emits `"NaN"`; invalid source pairs, dimensions, bands, and URLs exit nonzero.

Update the GDAL integration from direct-only inspection to reopen converted stores with `RasterStore`, call `level_meta`, and assert exact `read_window` samples. Task 4 adds `render_tile` assertions to this same integration file.

- [ ] **Step 6: Run all local/HTTP/CLI checks**

```bash
cargo test -p zarra --all-features --locked
cargo test -p zarra-cli --test cli --all-features --locked
cargo check -p zarra --no-default-features
bash scripts/test-gdal.sh
```

Expected: all pass, including real converted-store queries in the GDAL job.

- [ ] **Step 7: Commit querying**

```bash
git add .
git commit -m "feat: query sharded raster stores locally and over http"
```

---

## Task 4: Golden tile rendering and `zarra tile`

**Files:**
- Create: `crates/zarra/src/render.rs`, `crates/zarra/tests/render.rs`
- Create: `crates/zarra-cli/src/tile.rs`
- Modify: `crates/zarra/src/lib.rs`, `crates/zarra-cli/src/main.rs`
- Modify: `crates/zarra-cli/tests/{cli,gdal}.rs`

**Interfaces:**
- Consumes: `RasterStore`, manifest levels/ranges/nodata, and `Source`.
- Produces: `TileCoord`, `Tile`, `render_tile`, and CLI `zarra tile` exactly as `SPEC.md §§8-9`.
- Internal: `Ancestor`, `RenderSelection`, `ancestor`, `pack_data_tile`, `encode_rgba`; unit tests live beside the private packer and integration tests use `render_tile`.

- [ ] **Step 1: Write golden packing and overzoom tests first**

Inside `render.rs`'s `#[cfg(test)]` module, create small synthetic `Chunk` values and assert exact RGBA bytes for:

- RG16 low/mid/high/clamped values, R=high byte/G=low byte/B=0/A=255.
- RGB8 independent per-band ranges and repeated-band grayscale.
- Declared numeric nodata and NaN transparency, zero-filled transparent pixels, and `Empty` when all pixels are transparent.
- Only the first selected band controls alpha; a nodata value in a later band does not clear an otherwise valid first-band pixel.
- Nearest-neighbor subwindow sampling at factor 2.

In `crates/zarra/tests/render.rs`, decode output with `png::Decoder` and verify 16×16 reference output, including pixel (2,9) normalized from 2009/63063. Test invalid bands/coords, maxzoom, `+1`, `+12`, `+13`, and zoom 22. For the offset case, make a temp store whose level 2 is a copy of level 0 and whose level-2 manifest offset is (4,3); rendering z2/x4/y3 must succeed against array chunk (0,0), while valid z2/x0/y0 returns `Tile::Empty` before the offset. Also use a metadata-only sparse store to prove an absent shard returns `Tile::Empty`.

Run `cargo test -p zarra --test render`. Expected: FAIL because rendering is absent.

- [ ] **Step 2: Implement XYZ, band, ancestor, and offset resolution**

Implement `TileCoord::validate` using checked `1u32 << z` after requiring `z <= 22`. Parse all external numbers with Rust `parse`, never `atoi` behavior. CLI tile and the later HTTP route both call Task 3's `parse_band_list`; this module owns only the tile-specific count/range checks.

Resolve empty selections from the manifest (first band or first up to three); explicit selections have 1..=3 valid indices and may repeat. One band forces RG16 and that band's native range; two/three force RGB8. Reject invalid/empty tokens rather than reverting to defaults.

Use Reference Implementation C for ancestor selection. Subtract the level offset with checked arithmetic and return `Tile::Empty` for a negative/out-of-grid/sparse chunk. Never probe an unrelated intermediate level.

- [ ] **Step 3: Implement exact packing and PNG encoding**

Port `raster_pack_data_tile` from `server/routes/raster.h:204-278`, preserving center-of-pixel nearest sampling, clamping, rounding, and first-band alpha. Make NaN samples always transparent; infinite non-nodata values clamp to the appropriate endpoint. Later-band NaN encodes as zero in that channel without changing alpha unless it is the first selected band.

Use Reference Implementation D for color-type-6, 8-bit PNG with `Compression::Fast`. `render_tile` returns `Tile::Empty` when `painted == 0` and `Tile::Png` otherwise.

Run core packing/reference tests. Expected: every golden byte and deep-overzoom case passes.

- [ ] **Step 4: Wire `zarra tile` with no-clobber output**

Add the Clap command and call `render_tile`. For `Png`, write with `OpenOptions::create_new(true)` and remove a partial file if writing fails. For `Empty`, print `empty` and do not create `-o`. Print the output path on success.

Extend `tests/cli.rs`: fetch z0 PNG and check signature/IHDR; z0/x1 is rejected as invalid XYZ; a temporary metadata-only sparse store returns `empty` at z0/x0/y0 and creates no file; z13 succeeds; malformed bands fail; an existing output is not changed. Extend `tests/gdal.rs` to fetch native and overzoom PNGs from actual 2D/3D converted stores and decode their dimensions/pixels.

- [ ] **Step 5: Run rendering and full regression checks**

```bash
cargo test -p zarra --test render --all-features --locked
cargo test -p zarra-cli --test cli --all-features --locked
bash scripts/test-gdal.sh
```

Expected: all pass, including 2D/3D golden pixels and no-clobber behavior.

- [ ] **Step 6: Commit rendering**

```bash
git add .
git commit -m "feat: render raster zarr stores as xyz png tiles"
```

---

## Task 5: Loopback range server and real MapLibre viewer

**Files:**
- Create: `crates/zarra-cli/src/{serve,range,viewer}.rs`
- Create: `crates/zarra-cli/assets/{index.html,app.js,geo.js,styles.css,favicon.svg}`
- Create: `crates/zarra-cli/assets/vendor/{maplibre-gl.js,maplibre-gl.css,maplibre-LICENSE.txt}`
- Modify: `crates/zarra-cli/src/main.rs`, `crates/zarra-cli/Cargo.toml` (add `reqwest = { version = "0.12", default-features = false, features = ["blocking"] }` as a dev-dependency and keep `src/**`, `assets/**`, `tests/**`, `Cargo.toml`, `README.md` in the package include list)
- Create: `crates/zarra-cli/tests/serve.rs`
- Create: `example/{package.json,README.md,viewer.test.mjs}`
- Modify: `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: `RasterStore`, `render_tile`, manifest JSON, and local store files.
- Produces: `zarra serve <STORE> [--port <PORT>]`, routes in `SPEC.md §10`, and printed actual loopback URL (required for port-0 tests).
- Browser `geo.js` exports `tileCoords`, `rasterSource`, and `RASTER_OPACITY`; `app.js` imports and uses those exact functions.
- `example` scripts: `npm start` launches the same CLI against `../testdata/refpy.zarr`; `npm test` builds the CLI and runs Node integration tests.

- [ ] **Step 1: Vendor the verified browser dependency**

Run:

```bash
mkdir -p crates/zarra-cli/assets/vendor
curl -fL https://unpkg.com/maplibre-gl@4.7.1/dist/maplibre-gl.js \
  -o crates/zarra-cli/assets/vendor/maplibre-gl.js
curl -fL https://unpkg.com/maplibre-gl@4.7.1/dist/maplibre-gl.css \
  -o crates/zarra-cli/assets/vendor/maplibre-gl.css
curl -fL https://raw.githubusercontent.com/maplibre/maplibre-gl-js/v4.7.1/LICENSE.txt \
  -o crates/zarra-cli/assets/vendor/maplibre-LICENSE.txt
curl -fL https://unpkg.com/maplibre-gl@4.7.1/package.json \
  | python3 -c 'import json,sys; p=json.load(sys.stdin); assert p["version"]=="4.7.1" and p["license"]=="BSD-3-Clause"'
```

Do not copy PostHog, fonts, OSM basemap code, accounts, or the rest of Tilify's landing page.

- [ ] **Step 2: Write failing server and shared-browser-logic tests**

In `crates/zarra-cli/tests/serve.rs`, spawn `CARGO_BIN_EXE_zarra` with `serve <fixture> --port 0`, parse the printed URL, and kill/wait in a guard. Use the blocking `reqwest` dev-dependency for HTTP assertions; `send()` returns a response for ordinary 204/400/404 statuses, so tests do not need a second HTTP client or special status-error handling.

Test API JSON, native/deep-overzoom tiles, valid repeated and `b`-prefixed band selections, invalid-XYZ 400, invalid-band 400, true sparse 204, PNG IHDR, exact full/HEAD/suffix/bounded raw ranges, 416, 403 traversal/symlink escape, 404, 405, OPTIONS/CORS, and static assets. Create the sparse case in a temporary directory containing the root manifest and `0/zarr.json` but no shard; do not confuse it with z0/x1, which is invalid rather than empty. While that server is running, also invoke the real binary's `info --url <origin>/store` and `query --url <origin>/store` to prove the `zarrs_http` client and nested range server work together. Use a temporary symlink in the store to verify escape rejection on Unix; skip only that single assertion on non-Unix.

In `example/viewer.test.mjs`, resolve `REPO_ROOT` from `import.meta.url` and spawn `path.join(REPO_ROOT, "target", "debug", process.platform === "win32" ? "zarra.exe" : "zarra")` with `serve <absolute-canonical-fixture> --port 0`; parse stdout for the bound URL. Write equivalent `fetch` integration tests using `node:test` and `assert/strict`. Start the canonical fixture server for normal cases and a second temporary metadata-only server for the 204 case; kill/reap both in `after`. Import `../crates/zarra-cli/assets/geo.js` and test the same `tileCoords` math used by Tilify plus:

```js
const source = rasterSource(
  { tiles: "/raster/{z}/{x}/{y}.png", tile_size: 16, minzoom: 0, maxzoom: 0 },
  "http://127.0.0.1:8090",
);
assert.deepEqual(source, {
  type: "raster",
  tiles: ["http://127.0.0.1:8090/raster/{z}/{x}/{y}.png"],
  tileSize: 16,
  minzoom: 0,
  maxzoom: 0,
});
assert.equal(RASTER_OPACITY, 0.92);
```

Run the Node test before implementation. Expected: FAIL because the server/assets do not exist.

- [ ] **Step 3: Implement safe nested-object range serving**

In `range.rs`, adapt FGBA's `serve.rs` helpers:

- 16 KiB head cap, 30-second read/write timeouts, GET/HEAD only for content plus OPTIONS at the router.
- `Range::{None,Span,Unsatisfiable}` for `a-b`, `a-`, and `-b`, including empty files and 416.
- Percent-decode once; reject NUL, backslash, absolute paths, `.`/`..`, non-files, and canonical/symlink escapes. Serve only below the canonical store root.
- Stream exactly the requested span with `File::take`; accurate 200/206/416 and `Accept-Ranges`/`Content-Length`/`Content-Range` headers. Never read a whole shard just to answer a suffix range.

Add unit tests for `range_header`, percent decoding, and resolver edge cases in `crates/zarra-cli/tests/serve.rs`.

- [ ] **Step 4: Implement router, tile responses, and embedded assets**

In `serve.rs`, canonicalize/open the store before binding `127.0.0.1:port`, print the actual bound URL to stdout, and handle sequential connections. Routes:

- `/api/store`: serialize `manifest.to_value()`, insert `tiles`, and return no-store JSON.
- `/raster/<z>/<x>/<y>.png` with optional `bands`: parse coordinates strictly, call `parse_band_list` for the query value, then call `render_tile`; 200 PNG, 204 empty, 400 invalid input, 502 processing error. HEAD uses the same status/headers without a body.
- `/store/<key>`: call `range.rs`.
- Static allowlist from `viewer.rs`, including `/geo.js` and the MapLibre license.
- OPTIONS 204, unsupported methods 405, unknown routes 404. Send the specified CORS/cache/security headers (`X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`).

`viewer.rs` uses `include_bytes!`/`include_str!` for crate-local assets and explicit MIME types. Set the CLI package include list to `src/**`, `assets/**`, `tests/**`, `Cargo.toml`, and `README.md`; Task 6 verifies the packaged file list explicitly.

- [ ] **Step 5: Implement the actual viewer and npm launcher**

`geo.js` contains only pure shared logic: `RASTER_OPACITY = 0.92`, `tileCoords` using the source formula, and `rasterSource(manifest, origin="")` returning the exact source object tested above.

`app.js` fetches `/api/store`, creates a MapLibre map with a dark background, registers the real raster source/layer, and fits `extent` or fallback `[[-180,-85],[180,85]]`. Add `NavigationControl`, a labeled native fit button, metadata fields, pointer XYZ/zoom telemetry, and loading/error status through `aria-live`. No fabricated request ledger or hard-coded tile image.

`index.html` supplies a skip link, semantic header/main, a labeled map region, native controls, metadata definition list, and module scripts plus local MapLibre CSS. `styles.css` provides visible `:focus-visible`, responsive wide/mobile layouts, and `prefers-reduced-motion`. Keep the UI small; do not port FGBA's A5 rail/ledger or Tilify's upload page.

Write `example/package.json` with no dependencies, Node >=20, and:

```json
{
  "name": "zarra-raster-example",
  "version": "1.0.0",
  "private": true,
  "type": "module",
  "scripts": {
    "start": "cargo run -p zarra-cli -- serve ../testdata/refpy.zarr --port 8090",
    "dev": "cargo watch -x 'run -p zarra-cli -- serve ../testdata/refpy.zarr --port 8090'",
    "test": "cargo build -p zarra-cli && node --test viewer.test.mjs"
  },
  "engines": { "node": ">=20" }
}
```

`dev` requires optional `cargo-watch`; document that instead of installing it automatically. `start`/`test` need only Cargo and Node.

- [ ] **Step 6: Run HTTP and viewer integration tests**

```bash
cargo test -p zarra-cli --test serve --all-features --locked
npm --prefix example test
```

Expected: all server/range/PNG/static tests pass; `geo.js` tests use the same module the browser imports; the test server is killed and reaped.

Extend CI's stable job with `npm --prefix example test` and set up Node 20 using `actions/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020` with `node-version: 20`.

- [ ] **Step 7: Perform the browser smoke and commit**

Run `cargo run -p zarra-cli -- serve testdata/refpy.zarr --port 8090`, open the printed URL in a browser, and verify the `SPEC.md §11` smoke: nonblank real data, fit/zoom/keyboard, mobile layout, and no failed local asset/tile requests. Record any environment limitation instead of claiming a screenshot or WebGL run that did not happen.

```bash
git add .
git commit -m "feat: serve zarr tiles and maplibre viewer locally"
```

---

## Task 6: Package documentation, attribution, release checks, and final review

**Files:**
- Create: `README.md`, `ATTRIBUTION.md`, `CHANGELOG.md`, `RELEASING.md`, `LICENSE-APACHE`, `LICENSE-MIT`
- Create: `crates/zarra/README.md`, `crates/zarra-cli/README.md`, `example/README.md`
- Modify: `crates/{zarra,zarra-cli}/Cargo.toml` (final description, keywords, categories, and docs.rs metadata)
- Modify: `.github/workflows/ci.yml` for final documentation/package gates
- Review: all implementation files and `SPEC.md`

**Interfaces:**
- Produces the user-facing package contract; no new runtime API.
- Release order is `zarra` → wait for registry visibility → `zarra-cli`; there is no SQL dependency.

- [ ] **Step 1: Write accurate quickstarts and support documentation**

Root README includes:

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

Document optional `--url`, HTTP limitations, GDAL ≥3.13 requirement, `GDAL_BIN_DIR`, all five commands, array query layout, first-band alpha/RG16/RGB8 quantization, no-overwrite behavior, exact support matrix/non-goals, tests, and docs.rs/crates.io links only as intended destinations until published. Do not copy FGBA performance numbers or claim that a dry local fixture proves large-raster performance.

Per-crate READMEs explain library versus binary use. `example/README.md` states that `zarra serve` owns both range server and viewer, `example/` has no server/decoder, `dev` optionally needs `cargo-watch`, and Node tests build the binary.

- [ ] **Step 2: Add provenance, licenses, changelog, and release process**

Copy the Apache license text from FGBA. Use its MIT text with line 3 replaced by `Copyright (c) 2026 Zarra contributors`.

`ATTRIBUTION.md` cites:

- Tilify Everything files listed in Session Research as the behavior/test-fixture source.
- FGBA as the package/range-server structural reference; no FGB/A5 code in the runtime data path.
- Zarr/ZEP0002, `zarrs`/`zarrs_http` (MIT OR Apache-2.0), GDAL (MIT), `png` (MIT OR Apache-2.0), MapLibre 4.7.1 and Mapbox/glfx/d3 notices in the vendored license.

`CHANGELOG.md` uses Keep a Changelog/SemVer and one `0.1.0 - Unreleased` entry covering only implemented features. `RELEASING.md` includes clean-tree/tests, rechecking crate-name availability, `cargo package -p zarra`, publish core, wait for index, package/publish CLI, tag, docs.rs `http` feature check, and `cargo install zarra-cli` smoke. It does not instruct publishing a nonexistent SQL crate.

- [ ] **Step 3: Run formatting, lint, feature, docs, fixture, and package checks**

```bash
rustup toolchain install 1.91.0 --profile minimal
bash scripts/verify-fixtures.sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features --locked
cargo check -p zarra --no-default-features
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps
cargo +1.91.0 test --workspace --all-features --locked
npm --prefix example test
cargo package -p zarra --allow-dirty
cargo package -p zarra --list --allow-dirty
cargo package -p zarra-cli --list --allow-dirty
```

Expected: every command exits 0. The core package verifies successfully and includes its README/test fixture; the CLI package list includes `assets/index.html`, `assets/geo.js`, and all three vendored MapLibre files. Do not run a dependent CLI publish dry-run before core is published; the release document explains why.

After they pass locally, add the doc and core-package commands to CI. Keep stable, MSRV, GDAL, fixture, and Node jobs required.

- [ ] **Step 4: Run the full end-to-end and inspect the final diff**

```bash
bash scripts/test-gdal.sh
cargo run -p zarra-cli -- info testdata/refpy.zarr
cargo run -p zarra-cli -- query testdata/refpy.zarr \
  --level 0 --x 12 --y 12 --width 8 --height 8 > /tmp/zarra-window.json
cargo run -p zarra-cli -- tile testdata/refpy.zarr \
  --z 13 --x 0 --y 0 -o /tmp/zarra-overzoom.png
```

Use a new `/tmp/zarra-overzoom.png` path (remove an earlier one only after verifying it is this generated file). Assert the query JSON has 64 values and the PNG has 16×16 IHDR. Perform the browser smoke if it was not already completed against final assets.

Inspect `git status --short`, `git diff --stat`, and `git log --oneline --reverse`; review all prior commits plus the remaining documentation changes for scope, accidental generated stores, secrets, external-project edits, unused fields/state, and disconnected paths. There must be no changes in either reference repo.

- [ ] **Step 5: Self-review against the specification**

Check each acceptance item explicitly:

- [ ] GDAL conversion creates/reopens 2D and 3D canonical stores; repeat conversion is non-destructive.
- [ ] Local and HTTP reference values, offsets, windows, bands, sparse chunks, and NaN padding are exact.
- [ ] Golden RG16/RGB8/nodata/deep-overzoom/offset tests pass through zoom 22.
- [ ] Server nested ranges, HEAD, errors, traversal defenses, CORS, and static assets pass.
- [ ] Browser imports the same `geo.js` under test and consumes the real Rust tile route.
- [ ] Stable, 1.91.0 locked, GDAL 3.13.1, Node 20+, fixture, docs, and core packaging checks pass.
- [ ] Docs contain only observed results and the declared support matrix.

Fix any mismatch at its root cause; do not weaken a test or claim an unrun check.

- [ ] **Step 6: Commit package documentation and verification changes**

```bash
git add .
git commit -m "docs: package zarra for initial release"
git diff --stat HEAD~6..HEAD
git status --short
```

The diff covers the complete six-commit series. `git status --short` must be empty.

---

## Final Executor Report Format

Report only verified outcomes, with exact commands and counts where available:

1. Commits produced and files changed.
2. Stable/MSRV test totals and feature/doc/package checks.
3. GDAL 3.13.1 2D/3D conversion and no-clobber results.
4. Local/HTTP reference-query and golden-pixel results, including `maxzoom+13` and zoom 22.
5. Node server/viewer tests and whether the final WebGL browser smoke was actually observed.
6. Any residual limitation from `SPEC.md` (especially HTTP timeout/full-body/redirect behavior, GDAL resource bounds, and local-only publication) without presenting it as a defect fixed in v1.
