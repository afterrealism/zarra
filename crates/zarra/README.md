# zarra

`zarra` is the Rust library for the canonical multiscale Zarr v3 raster profile. It provides:

- GeoTIFF/COG conversion through GDAL 3.13+ command-line tools;
- strict root-manifest and level-metadata validation;
- local and optional keyed-HTTP stores, plus async storage through the `async` feature;
- chunk, window, neighborhood, and ordered/repeated-band queries;
- optional bounded read-only SQLite analytics through the `sql` feature;
- native and deep-overzoom XYZ rendering to RG16/RGB8 RGBA PNG tiles.

The `zarra-cli` package supplies the `zarra` binary and embedded viewer. See the [workspace README](https://github.com/afterrealism/zarra#readme) for the complete CLI/support contract and [SPEC.md](https://github.com/afterrealism/zarra/blob/main/SPEC.md) for the authoritative profile.

## Path use

Version 0.1.0 is Unreleased. crates.io publication is deferred until `zarrs 0.23.15` passes the upstream gate in the workspace release guide; the current full-SHA backport supports local/path use, not a registry-publication claim.

From another crate in this workspace, use:

```toml
[dependencies]
zarra = { path = "../zarra" }
```

The core's default feature is `local`, which retains native filesystem/GDAL
behavior. Enable `http` for HTTPS and loopback-HTTP stores, `async` for async
Zarr storage, and `sql` for shared read-only SQLite analytics. The standalone
browser crate uses `default-features=false, features=["async","sql"]`:

```toml
zarra = { path = "../zarra", features = ["http"] }
```

## Library example

```rust
use std::path::Path;
use zarra::{RasterStore, Result, TileCoord};

fn main() -> Result<()> {
    let mut store = RasterStore::open(Path::new("output.zarr"))?;
    let window = store.read_window(0, 12, 12, 8, 8)?;
    assert_eq!(window.values.len(), 8 * 8 * window.bands.len());

    let tile = zarra::render_tile(
        &mut store,
        TileCoord { z: 13, x: 0, y: 0 },
        &[],
    )?;
    assert!(matches!(tile, zarra::Tile::Png(_) | zarra::Tile::Empty));
    Ok(())
}
```

`zarra::convert` accepts a local input and an absent output directory with default settings. `zarra::convert_with_options` additionally accepts `ConvertOptions` for an optional maximum zoom, GDAL resampling, and ZSTD level 1–22. Both require `gdalinfo`, `gdalwarp`, and `gdal_translate`; `GDAL_BIN_DIR` selects their directory when they are not on `PATH`.

With the `http` feature, `RasterStore::open_url` accepts HTTPS and loopback HTTP. It rejects userinfo, query strings, fragments, unsupported schemes, and non-loopback plain HTTP. Upstream HTTP behavior provides no total-timeout, request/byte-budget, full-body avoidance, or HTTPS-downgrade guarantee.

Window values are row-major and band-interleaved `f64`; chunk values are band-planar. Sparse and outside-array window cells are NaN, while transport and decode failures remain errors. PNG rendering clamps and quantizes for display, and only the first selected band controls declared-nodata alpha.

The separate `zarra-wasm` crate reuses this core for direct signed-object-store browser rendering and SQL. It is not the native `zarra serve` demo or a second JavaScript decoder. See the workspace [Tilify integration guide](https://github.com/afterrealism/zarra/blob/main/docs/tilify-integration.md) for the actual build, signed URL/CORS lifecycle, and hard limits.

## Verification and license

Run `cargo test -p zarra --all-features --locked` and `cargo check -p zarra --no-default-features` from the workspace. The intended API documentation destination after publication is [docs.rs/zarra](https://docs.rs/zarra), built with the `async`, `http`, and `sql` features.

Licensed under Apache-2.0 OR MIT. See the workspace `LICENSE-APACHE`, `LICENSE-MIT`, and `ATTRIBUTION.md` files.
