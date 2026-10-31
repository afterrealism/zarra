# Tilify integration: native Zarra and standalone browser WASM

This note separates Zarra's existing native CLI/server from the standalone
browser artifact used by Tilify Everything. For the complete service contract,
see Tilify's `docs/architecture.md`; the canonical raster profile remains in
[SPEC.md](../SPEC.md).

## Roles: native package versus browser artifact

| Artifact | Current role | Boundary |
|---|---|---|
| `zarra` core with default `local` feature | Native filesystem conversion, reads, rendering, and optional shared SQL | Default native behavior remains supported |
| `zarra-cli` / `zarra` binary | `convert`, `info`, `query`, `sql`, `tile`, and loopback `serve`; Tilify uses native `zarra convert` for upload ETL | Not the browser bundle and not a production multi-tenant service |
| `zarra serve` + `example/` | Existing sequential loopback demo with its explicit tile/range/SQL/upload routes and embedded MapLibre viewer | These demo routes are not Tilify's direct-browser API |
| `zarra-wasm` | Standalone Rust/WASM + worker/JavaScript client using `zarra` with `default-features=false, features=["async","sql"]` | Supports the canonical profile, not arbitrary Zarr or a second JavaScript decoder |

The workspace MSRV is **Rust 1.91**. Native conversion still requires GDAL
3.13+ CLI tools; GDAL 3.13.1 is the tested compatibility version. Registry
release order remains `zarra` → `zarra-cli`; `zarra-wasm` is built from source
with wasm-pack and is not a separate registry release step.

## Native Tilify ETL and retained behavior

Tilify runs the native processor and uploads every regular file in the
resulting canonical store:

```bash
zarra convert input.tif --output store.zarr
zarra info store.zarr
```

The object prefix is `rasters/<layer-key>/<generation>.zarr`, with root
`zarr.json`, per-level `zarr.json`, and canonical numeric `c/...` shard keys.
A complete raster has `format=zarr-v3`, `queryable=true`, and retains manifest
zoom/extent/band metadata.

The existing native commands and local demo are unchanged in role:

```bash
zarra query output.zarr --level 0 --x 0 --y 0 --width 8 --height 8
zarra sql output.zarr "SELECT band, AVG(value) FROM raster GROUP BY band"
zarra tile output.zarr --z 0 --x 0 --y 0 -o tile.png
zarra serve output.zarr --port 8090 --upload-root uploads
```

Native `serve` can render tiles, answer its demo SQL endpoint, serve nested
object ranges, and convert local uploads when explicitly configured. Tilify
does not use those routes for uploaded-dataset browser rendering or SQL; its
app is only the control plane and signer.

## Build the actual standalone browser package

From the Zarra repository root:

```bash
rustup target add wasm32-unknown-unknown
npx --yes wasm-pack@0.15.1 build crates/zarra-wasm \
  --target web --release -- --locked
```

Default wasm-pack output is generated `crates/zarra-wasm/pkg/`, not source to
commit. Tilify's `scripts/build-web.mjs` supplies a temporary `--out-dir`,
fingerprints the generated `pkg/` plus `js/` files, and copies the result into
its ignored, versioned `web/assets/runtimes/` layout.

The package contains:

- Rust `BrowserRaster.open(get)`, `manifestJson()`, `renderTile(...)`,
  `queryJson(...)`, and `close()`;
- an `AsyncReadableStorageTraits` adapter over the JavaScript `get` callback;
- `js/object-store.js` for per-generation/key signed URL caching and refresh;
- `js/worker.js` and `js/client.js` for serialized sessions and the MapLibre
  protocol.

Tilify supplies its shared generation-scoped signer to the same WASM API. No
native filesystem, blocking `zarrs_http`, SQL.js, DuckDB, or second JavaScript
Zarr decoder is introduced. Both native and browser SQL use Zarra's core
`sql` feature and SQLite implementation.

## Signed per-key URL lifecycle and CORS

The control plane signs relative paths beneath the registry-selected Zarr
prefix. Accepted paths are root `zarr.json`, canonical `<level>/zarr.json`, and
canonical numeric `<level>/c/...` chunks; callers cannot choose another bucket,
host, generation, arbitrary prefix, or traversal path. One signing call accepts
1–64 paths, returns `expires_at` and the generation, and is marked
`Cache-Control: no-store` and `Referrer-Policy: no-referrer`. Only GET is
signed.

The direct data path is:

```text
openRaster({key, signEndpoint, generation})
  → sign/refresh each relative key
  → direct object GET (full metadata or exact range)
  → async Zarr/shards → shared PNG renderer or bounded SQLite window
```

Lifecycle and error rules:

1. Full metadata reads require `200`; partial reads require `206` with the
   exact requested `Content-Range` and body length. A `200` answer to a partial
   request is an error, never a whole-object fallback.
2. Object size probes use `Range: bytes=0-0`; `416` with `Content-Range:
   bytes */0` represents an empty object for that probe. Other `416` responses
   remain errors. Multi-range trait calls become separate single-range GETs.
3. A `404` can represent a genuinely sparse chunk. Transport, `500`, metadata,
   and decode failures never silently become sparse data or transparent tiles.
4. URLs are cached by generation + key and refreshed at
   `expires_at - min(60 seconds, 20% of TTL)`. A `401`/`403` causes one refresh
   and retry. A changed generation invalidates the session rather than mixing
   objects from different stores.
5. Requests use `credentials: "omit"`, `redirect: "error"`, and
   `cache: "no-store"`. Production signed URLs require HTTPS; HTTP is limited
   to loopback tests. Signed URLs are not logged, persisted, inserted into DOM
   state, or sent to analytics.

Bucket CORS must allow the exact Tilify web origins, GET, and `Range`, and expose
`Accept-Ranges`, `Content-Length`, and `Content-Range`. Unsigned preflight
`OPTIONS` must permit Range; it does not need to be signed or listed as an
object method. Presigning does not bypass CORS. Disposable MinIO evidence is
not an Alibaba OSS compatibility claim.

## Rendering and SQL support/caps

`renderTile` supports the canonical Zarr v3 profile, XYZ zoom 0–22, native and
deep-overzoom reads, ordered/repeated band selections, and the existing
RG16/RGB8/nodata-alpha semantics. Empty tiles become valid transparent PNGs for
MapLibre. The custom protocol is exactly:

```text
zarra:///{z}/{x}/{y}.png
```

and returns `{data: ArrayBuffer}` through MapLibre `addProtocol`. PNG output is
quantized for display, not lossless scientific sample export.

Browser and native SQL expose one relation per selected window:

```text
raster(x INTEGER, y INTEGER, band INTEGER, value REAL)
```

NaN and declared nodata samples become `NULL`. SQL accepts one read-only
`SELECT`/`WITH`; SQLite `query_only`, `Statement::readonly`, and
single-statement preparation reject writes and multiple statements. The
browser supplies a monotonic `performance.now()` clock to the shared interrupt
logic.

| Limit | Native and browser shared behavior |
|---|---:|
| Staged source band values | 1,048,576 |
| SQL execution interrupt | 10 seconds |
| Result rows | 1,000 |
| Compact result JSON, including metadata/escaping | 8 MiB |
| SQL text SQLite limit | 64 KiB |

The highest store level is the default; `width` and `height` must be supplied
together or omitted together for the whole level. Source/result caps are not a
whole-process memory guarantee. Arbitrary Zarr versions, layouts, codecs,
mixed band dtypes/nodata, persistent analytics sidecars, and remote conversion
remain outside the package contract.

## QGIS and deletion in Tilify

QGIS lists every object, enforces its default **1 GiB** download cap before
signing/fetching, signs at most 64 paths per batch, and downloads directly with
four workers. Exact path/size verification and a metadata-only manifest gate a
managed `<layer-key>/<generation>` cache; partial generations are removed and
complete ones can be reused. Saved projects reference only the local Zarr
directory. Raster requires **GDAL 3.13+** and fails clearly on older GDAL;
signed URLs never enter project files, QSettings, history, logs, or manifests.

Tilify owner deletion marks the registry generation `deleting`, removes the raw
source prefix and complete `rasters/<key>/<generation>.zarr` prefix, then erases
the registry row and returns 204. Storage failure leaves a retryable deleting
record and returns 502. Anonymous TTL and failed/stale-upload cleanup use the
same registered-prefix lifecycle. Zarra itself does not implement this
multi-tenant deletion service.

## Local verification

Run the native regression suite and actual standalone artifact checks; native
server/browser-demo checks alone do not prove WASM support:

```bash
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
npm --prefix example test
bash scripts/test-gdal.sh
cargo +1.91.0 test --workspace --all-features --locked
```

The direct browser tests use real fixture bytes and a separate-origin mock
store to cover full/range/error responses, sparse-versus-error behavior,
native/overzoom/band/empty PNG parity, SQL/nodata/caps/timeouts, generation
checks, and expiry refresh. `scripts/test-gdal.sh` retains the real GDAL 3.13.1
native conversion/upload tests. Tilify's disposable E2E covers the complete
upload → sign → direct object → map/SQL → deletion journey.
